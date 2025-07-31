use std::{collections::{HashMap, HashSet}, fs::OpenOptions};

use async_std::{fs::{create_dir_all, File}, stream::StreamExt, task};
use futures::stream::FuturesUnordered;

use super::*;

use crate::{instance::download::libs::LibsData, utils::metacache, websocket::messages::operation::process::{FileStatus, ProcessTarget}};

impl<'a> LibsData<'a> {
    pub async fn download_missing_libs(
        version_libs: HashMap<&str, (String, String, &str)>,
        paths: &InstancePaths,
        ws_status: OperationWsMessageLocked<'a>,
    ) -> Result<Vec<String>, String> {
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

        let mut downloaded_libs: HashSet<LibInfo> = HashSet::new();
        let mut libs_paths = Vec::new();

        if let Some(libraries) = metacache["libraries"].as_array() {
            let mut futures = FuturesUnordered::new();

            for (k, v) in version_libs.iter() {
                let lib_hash = k.to_string();
                let lib_name = v.0.to_string();
                let lib_path = v.1.to_string();
                let lib_url = v.2.to_string();

                if libs_paths.is_empty() {
                    libs_paths.push(format!("{}/{}", paths.libs().display(), lib_path));
                } else {
                    libs_paths.push(format!(":{}/{}", paths.libs().display(), lib_path));
                }

                if !libraries
                    .iter()
                    .any(|lib| lib["hash"].as_str() == Some(&lib_hash))
                {
                    let libs_path = paths.libs().clone();

                    futures.push(task::spawn(async move {
                        match Self::download_lib(
                            &lib_name,
                            &lib_path,
                            &lib_url,
                            &lib_hash,
                            &libs_path.to_str().unwrap(),
                        )
                        .await
                        {
                            Ok(lib_info) => Some(lib_info),
                            Err(e) => {
                                println!("{e}");
                                None
                            }
                        }
                    }));

                    if futures.len() >= 100 {
                        Self::process_futures(
                            &mut futures,
                            &mut downloaded_libs,
                            version_libs.len(),
                            Arc::clone(&ws_status),
                        )
                        .await;
                    }
                }
            }

            Self::process_futures(
                &mut futures,
                &mut downloaded_libs,
                version_libs.len(),
                ws_status,
            )
            .await;
        }

        Self::register_libs(downloaded_libs, metacache, paths).await;
        Ok(libs_paths)
    }

    async fn process_futures(
        futures: &mut FuturesUnordered<async_std::task::JoinHandle<std::option::Option<LibInfo>>>,
        downloaded_libraries: &mut HashSet<LibInfo>,
        max: usize,
        ws_status: OperationWsMessageLocked<'a>,
    ) {
        let mut ws_status = ws_status;

        while let Some(result) = futures.next().await {
            if let Some(asset_info) = result {
                    ws_status = ws_status
                        .update_determinable(
                        STAGE_TYPE,
                        Some(ProcessTarget::file(
                            asset_info.name.clone(),
                            FileStatus::Downloaded,
                        )),
                        downloaded_libraries.len(),
                        max,
                    )
                    .await;

                downloaded_libraries.insert(asset_info);
            }
        }
    }

    async fn download_lib(
        lib_name: &String,
        lib_path: &String,
        lib_url: &str,
        lib_hash: &str,
        libs_path: &str,
    ) -> Result<LibInfo, String> {
        if let Some(pos) = lib_path.rfind('/') {
            let dir_path = format!("{}/{}", libs_path, &lib_path[..pos].to_string());
            println!("Creating directory: {}", dir_path);

            if let Err(e) = create_dir_all(&dir_path).await {
                println!("Failed to create directory: {e}");
                // continue;
            }

            match surf::get(lib_url).await {
                Ok(mut response) => {
                    println!("Downloading library \"{}\"", &lib_name);

                    let mut file = File::create(format!("{}/{}", libs_path, &lib_path))
                        .await
                        .unwrap();

                    async_std::io::copy(&mut response, &mut file).await.unwrap();

                    let lib_info = LibInfo {
                        hash: lib_hash.to_string(),
                        name: lib_name.to_string(),
                        path: lib_path.to_string(),
                    };
                    return Ok(lib_info);
                }
                Err(e) => {
                    println!("Failed to download library: {e}");
                    return Err(e.to_string());
                }
            }
        } else {
            println!("Failed to parse path: {}", lib_path);
            Err(format!("Failed to parse path"))
        }
    }
}
