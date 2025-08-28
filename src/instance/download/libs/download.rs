use std::collections::HashSet;

use async_std::task;
use futures::{stream::FuturesUnordered, StreamExt};

use crate::{utils::download::{buffer::BufferPool, Download}, websocket::messages::operation::process::{FileStatus, ProcessTarget}};

use super::*;

const CONCURRENT_TASKS_COUNT: usize = 100;
const CONCURRENT_BUFFERS_SIZE: usize = 16 * 1024; // 16 KiB each

impl<'a, 'b> LibsData<'a, 'b> {
    pub async fn download_missing_libs(
        downloadable_libs: Vec<LibInfo>,
        ws_status: OperationWsMessageLocked<'a>,
        db: &'a db::Database,
    ) -> Result<SyncResult, String> {
        let downloadable_libs_count = downloadable_libs.len();
        let mut downloaded_libs: HashSet<LibInfo> = HashSet::new();
        let mut classpaths = Vec::new();

        let mut futures = FuturesUnordered::new();
        let download_buffers_pool = Arc::new(BufferPool::new(
            CONCURRENT_TASKS_COUNT,
            CONCURRENT_BUFFERS_SIZE,
        ));

        let cached = match sqlx::query_as!(
            LibInfo,
            r#"
            SELECT name, hash, path, url, native
            FROM libraries
            ORDER BY name
            "#
        )
        .fetch_all(&db.pool)
        .await
        {
            Ok(cached) => cached,
            Err(e) => return Err(e.to_string()),
        };

        let missing = Self::get_missing_by_hash(cached, downloadable_libs, &mut classpaths);
        for missing_lib in missing.into_iter() {
            let current_buffer = Arc::clone(&download_buffers_pool);

            let download_info = Download::new(
                PathBuf::from(missing_lib.path.clone()),
                missing_lib,
                current_buffer,
            );

            futures.push(task::spawn(async move {
                match download_info.download_with_checksum().await {
                    Ok(lib_info) => Some(lib_info),
                    Err(e) => {
                        println!("{e}");
                        None
                    }
                }
            }));

            if futures.len() >= CONCURRENT_TASKS_COUNT {
                Self::process_futures(
                    &mut futures,
                    &mut downloaded_libs,
                    downloadable_libs_count,
                    ws_status.clone(),
                )
                .await;
            }
        }

        Self::process_futures(
            &mut futures,
            &mut downloaded_libs,
            downloadable_libs_count,
            ws_status,
        )
        .await;

        match Self::register_libs(&mut downloaded_libs, &db).await {
            Ok((classpaths, natives_paths)) => Ok(SyncResult {
                classpaths,
                natives_paths,
            }),
            Err(e) => Err(e.to_string()),
        }
    }

    // Useful util function to insert delimeter
    fn add_to_classpaths(classpaths: &mut Vec<String>, value: String) {
        if classpaths.is_empty() {
            classpaths.push(value);
        } else {
            classpaths.push(format!(":{}", value))
        }
    }

    /// Checks for missing libs and extracts classpaths for launch
    fn get_missing_by_hash(
        downloaded: Vec<LibInfo>,
        needed: Vec<LibInfo>,
        classpaths: &mut Vec<String>,
    ) -> Vec<LibInfo> {
        let have: HashSet<&str> = downloaded.iter().map(|l| l.hash.as_str()).collect();
        let mut missing: Vec<LibInfo> = Vec::new();

        for lib in needed.into_iter() {
            if !have.contains(lib.hash.as_str()) {
                missing.push(lib);
            } else {
                Self::add_to_classpaths(classpaths, lib.path);
            }
        }

        missing
    }

    async fn process_futures(
        futures: &mut futures::stream::FuturesUnordered<
            async_std::task::JoinHandle<std::option::Option<LibInfo>>,
        >,
        downloaded_libraries: &mut HashSet<LibInfo>,
        max: usize,
        ws_status: OperationWsMessageLocked<'a>,
    ) {
        let mut ws_status = ws_status;

        while let Some(result) = futures.next().await {
            if let Some(lib_info) = result {
                ws_status = ws_status
                    .update_determinable(
                        STAGE_TYPE,
                        Some(ProcessTarget::file(
                            lib_info.name.clone(),
                            FileStatus::Downloaded,
                        )),
                        downloaded_libraries.len(),
                        max,
                    )
                    .await;

                downloaded_libraries.insert(lib_info);
            }
        }
    }
}
