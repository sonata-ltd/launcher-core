use std::{collections::HashSet, fs::OpenOptions, path::PathBuf, sync::Arc};

use async_std::{
    fs::{create_dir_all, File},
    io::ReadExt,
    stream::StreamExt,
    task,
};
use futures::{stream::FuturesUnordered, AsyncWriteExt};
use sha1::{Digest, Sha1};
use surf::Url;

use super::*;

use crate::{
    instance::download::libs::LibsData,
    utils::{
        download::{buffer::BufferPool, MAX_REDIRECT_COUNT},
        metacache,
    },
    websocket::messages::operation::process::{FileStatus, ProcessTarget},
};

const CONCURRENT_TASKS_COUNT: usize = 100;
const CONCURRENT_BUFFERS_SIZE: usize = 16 * 1024; // 16 KiB each

impl<'a, 'b> LibsData<'a, 'b> {
    pub async fn download_missing_libs(
        downloadable_libs: Vec<LibInfo>,
        paths: &InstancePaths,
        ws_status: OperationWsMessageLocked<'a>,
    ) -> Result<SyncResult, String> {
        let metacache_file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .open(paths.metacache_file())
            .unwrap();

        let metacache: serde_json::Value = match serde_json::from_reader(&metacache_file) {
            Ok(value) => value,
            Err(_) => match metacache::recreate(paths.metacache_file()) {
                Ok((_file, value)) => value,
                Err(e) => {
                    println!("Failed to recreate metacache file: {}", e);
                    return Err(format!("Failed to recreate metacache file: {e}"));
                }
            },
        };

        let downloadable_libs_count = downloadable_libs.len();
        let mut downloaded_libs: HashSet<LibInfo> = HashSet::new();
        let mut libs_paths = Vec::new();

        let mut futures = FuturesUnordered::new();
        let download_buffers_pool = Arc::new(BufferPool::new(
            CONCURRENT_TASKS_COUNT,
            CONCURRENT_BUFFERS_SIZE,
        ));
        let save_path = Arc::new(paths.libs().to_path_buf());

        if let Some(libraries) = metacache["libraries"].as_array() {
            for mut lib in downloadable_libs.into_iter() {
                // Insert delimeter
                let lib_save_path = format!("{}/{}", paths.libs().display(), lib.path);
                lib.save_path = Some(PathBuf::from(lib_save_path.clone()));
                if libs_paths.is_empty() {
                    libs_paths.push(lib_save_path);
                } else {
                    libs_paths.push(format!(":{}", lib_save_path));
                }

                if !libraries
                    .iter()
                    .any(|l| l["hash"].as_str() == Some(&lib.hash))
                {
                    let save_path = Arc::clone(&save_path);
                    let current_buffer = Arc::clone(&download_buffers_pool);

                    futures.push(task::spawn(async move {
                        match Self::download_lib(lib, save_path, current_buffer).await {
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
                } else {
                    downloaded_libs.insert(lib);
                }
            }
        }

        Self::process_futures(
            &mut futures,
            &mut downloaded_libs,
            downloadable_libs_count,
            ws_status,
        )
        .await;

        match Self::register_libs(&downloaded_libs, metacache, paths).await {
            Ok(_) => {
                let mut natives = Vec::new();

                for lib in downloaded_libs {
                    if lib.is_native() {
                        if let Some(save_path) = lib.save_path {
                            natives.push(save_path);
                        }
                    }
                }

                Ok(SyncResult {
                    classpaths: libs_paths,
                    natives,
                })
            }
            Err(e) => Err(e.to_string()),
        }
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

    async fn download_lib(
        lib_info: LibInfo,
        libs_path: Arc<PathBuf>,
        buffers_pool: Arc<BufferPool>,
    ) -> Result<LibInfo, String> {
        let libs_path = libs_path.as_ref().display().to_string();

        if let Some(pos) = lib_info.path.rfind('/') {
            let dir_path = format!("{}/{}", libs_path, &lib_info.path[..pos].to_string());

            if let Err(e) = create_dir_all(&dir_path).await {
                println!("Failed to create directory: {e}");
                // continue;
            }

            let mut current_url = lib_info.url.clone();
            let mut redirect_count: usize = 0;

            loop {
                // Ask for identity encoding
                let req = surf::get(&current_url).header("Accept-Encoding", "identity");
                let mut resp = match req.await {
                    Ok(data) => data,
                    Err(e) => return Err(format!("Request error for {}: {}", current_url, e)),
                };

                // Handle redirect from server
                if resp.status().is_redirection() {
                    let status = resp.status();
                    let location = resp.header("Location").map(|v| v.last());

                    if redirect_count >= MAX_REDIRECT_COUNT {
                        return Err(format!("Too many redirects when fetcing {}", current_url));
                    }

                    let location = match location {
                        Some(loc) => loc.as_str(),
                        None => {
                            return Err(format!(
                                "Redirect (status {}) without Location for {}.",
                                status, current_url
                            ));
                        }
                    };

                    // Resolve relative Location
                    let base = Url::parse(&current_url)
                        .map_err(|e| format!("Base URL parse error {}: {}", current_url, e))?;
                    let next_url = match Url::parse(&location) {
                        Ok(u) => u, // Absolute
                        Err(_) => base.join(&location).map_err(|e| {
                            format!("Failed to join {} + {}: {}", base, location, e)
                        })?,
                    };

                    println!("Next url: {}", next_url.to_string());
                    current_url = next_url.to_string();
                    redirect_count += 1;

                    continue;
                }

                // Not a redirect -> proceed to download
                println!("Downloading \"{}\" from URL {}", lib_info.name, current_url);

                if !resp.status().is_success() {
                    return Err(format!(
                        "HTTP error {} when fetching {}",
                        resp.status(),
                        current_url
                    ));
                }

                // Prepare file
                let full_save_path = match lib_info.save_path {
                    Some(ref path) => path,
                    None => return Err(format!("No save path defined for this library")),
                };
                let mut file = match File::create(&full_save_path).await {
                    Ok(f) => f,
                    Err(e) => {
                        return Err(format!(
                            "Failed to create file {}: {}",
                            full_save_path.display(),
                            e
                        ))
                    }
                };

                // Stream -> hasher + file at once with bytes read logging
                let mut hasher = Sha1::new();
                let mut guard = buffers_pool.acquire().await;
                let buf = guard.as_mut_slice();
                let mut total_read: usize = 0;
                loop {
                    let n = resp.read(buf).await.map_err(|e| e.to_string())?;
                    if n == 0 {
                        break;
                    }

                    hasher.update(&buf[..n]);
                    file.write_all(&buf[..n]).await.map_err(|e| e.to_string())?;
                    total_read += n;
                }

                if total_read == 0 {
                    return Err(format!("Read 0 bytes from {}", current_url));
                }

                let calculated_sha1 = format!("{:x}", hasher.finalize());
                let expected = lib_info.hash.to_lowercase();

                if calculated_sha1 != expected {
                    return Err(format!("SHA1 mismatch at {}", current_url));
                } else {
                    return Ok(lib_info);
                }
            }
        } else {
            println!("Failed to parse path: {}", lib_info.path);
            Err(format!("Failed to parse path"))
        }
    }
}
