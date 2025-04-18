use async_std::task;
use async_std::{
    fs::{create_dir_all, File},
    io::WriteExt,
};
use chrono::Utc;
use futures::stream::FuturesUnordered;
use futures::StreamExt;
use serde_json::{self, json};
use std::collections::{HashMap, HashSet};
use std::fs::OpenOptions;
use tide_websockets::WebSocketConnection;

use crate::instance::Paths;
use crate::utils::metacache;
use crate::websocket::messages::operation::event::OperationUpdate;
use crate::websocket::messages::operation::process::{FileStatus, ProcessStatus, ProcessTarget};
use crate::websocket::messages::operation::stage::{OperationStage, StageResult, StageStatus};
use crate::websocket::messages::operation::OperationMessage;
use crate::websocket::messages::{BaseMessage, WsMessage, WsMessageType};

const STAGE_TYPE: OperationStage = OperationStage::DownloadLibs;

pub async fn sync_libs(
    manifest: &serde_json::Value,
    paths: &Paths,
    ws: &WebSocketConnection,
) -> Result<(String, Vec<String>), String> {
    let msg: WsMessage = OperationMessage {
        base: BaseMessage {
            message_id: "todo",
            operation_id: Some("todo"),
            request_id: Some("todo"),
            timestamp: Utc::now(),
            correlation_id: None,
        },
        data: OperationUpdate::Determinable {
            stage: OperationStage::DownloadLibs,
            status: ProcessStatus::Started,
            target: None,
            current: 0,
            total: 0,
        }
        .into(),
    }
    .into();

    msg.send(&ws).await.unwrap();

    let done_paths = match extract_manifest_libs(manifest, "osx", paths, ws).await {
        Ok(paths) => paths,
        Err(e) => return Err(e),
    };

    println!("Download Finished");

    let msg: WsMessage = OperationMessage {
        base: BaseMessage {
            message_id: "todo",
            operation_id: Some("todo"),
            request_id: Some("todo"),
            timestamp: Utc::now(),
            correlation_id: None,
        },
        data: OperationUpdate::Completed(StageResult {
            status: StageStatus::Completed,
            stage: STAGE_TYPE,
            duration_secs: 0.0,
            error: None,
        })
        .into(),
    }
    .into();

    msg.send(&ws).await.unwrap();

    Ok((format!("{}/.sonata/libraries/", paths.root), done_paths))
}

async fn extract_manifest_libs(
    manifest: &serde_json::Value,
    current_os: &str,
    paths: &Paths,
    ws: &WebSocketConnection,
) -> Result<Vec<String>, String> {
    // Hashmap contains: hash, (name, path, url)
    let mut version_libs: HashMap<&str, (String, String, &str)> = HashMap::new();

    println!("Extraction libraries...");

    if let Some(libraries) = manifest["libraries"].as_array() {
        for lib in libraries {
            let lib_name = lib["name"].as_str();

            let allow_lib = if let Some(rules) = lib["rules"].as_array() {
                rules.iter().any(|rule| {
                    if let Some(action) = rule["action"].as_str() {
                        if action == "allow" {
                            if let Some(os) = rule["os"].as_object() {
                                if os.get("name").and_then(|name| name.as_str()) == Some(current_os)
                                {
                                    return true;
                                }
                            } else {
                                return true;
                            }
                        }
                    }

                    false
                })
            } else {
                true
            };

            if allow_lib {
                let lib_path = lib["downloads"]["artifact"]["path"].as_str();
                let lib_url = lib["downloads"]["artifact"]["url"].as_str();
                let lib_hash = lib["downloads"]["artifact"]["sha1"].as_str();

                if let (Some(lib_name), Some(lib_path), Some(lib_url), Some(lib_hash)) =
                    (lib_name, lib_path, lib_url, lib_hash)
                {
                    version_libs.insert(
                        lib_hash,
                        (lib_name.to_string(), lib_path.to_string(), lib_url),
                    );
                }
            }

            // Check for classifiers
            if let Some(natives) = lib["natives"].as_object() {
                for (k, native_name) in natives {
                    if k == current_os {
                        if let Some(classifiers) = lib["downloads"]["classifiers"].as_object() {
                            for (name, v) in classifiers {
                                if name == native_name {
                                    let lib_path = v["path"].as_str();
                                    let lib_url = v["url"].as_str();
                                    let lib_hash = v["sha1"].as_str();

                                    if let (
                                        Some(lib_name),
                                        Some(lib_path),
                                        Some(lib_url),
                                        Some(lib_hash),
                                    ) = (lib_name, lib_path, lib_url, lib_hash)
                                    {
                                        println!("Found: {}", lib_name);
                                        if let Some(updated) = version_libs.insert(
                                            lib_hash,
                                            (lib_name.to_string(), lib_path.to_string(), lib_url),
                                        ) {
                                            println!("REWRITED: {:#?}", updated);
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            } else {
            }
        }
    }

    if let Some(client_url) = manifest["downloads"]["client"]["url"].as_str() {
        let name = manifest["id"].as_str().unwrap();
        let name = name.to_owned() + "-client.jar";
        let path = "com/mojang/minecraft/".to_owned() + &name;
        let hash = manifest["downloads"]["client"]["sha1"].as_str().unwrap();

        version_libs.insert(hash, (name, path, client_url));
    }

    println!("{:#?}", version_libs);
    match download_missing_libs(version_libs, paths, ws).await {
        Ok(paths) => Ok(paths),
        Err(e) => Err(e),
    }
}

#[derive(Eq, Hash, PartialEq, Debug)]
struct LibInfo {
    hash: String,
    name: String,
    path: String,
}

async fn download_missing_libs<'a>(
    version_libs: HashMap<&str, (String, String, &str)>,
    paths: &'a Paths,
    ws: &WebSocketConnection,
) -> Result<Vec<String>, String> {
    let metacache_file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .open(&paths.metacache_file)
        .unwrap();

    let metacache: serde_json::Value = match serde_json::from_reader(&metacache_file) {
        Ok(value) => value,
        Err(_) => match metacache::recreate(&paths.metacache_file) {
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
                libs_paths.push(format!("{}/{}", paths.libs, lib_path));
            } else {
                libs_paths.push(format!(":{}/{}", paths.libs, lib_path));
            }

            if !libraries
                .iter()
                .any(|lib| lib["hash"].as_str() == Some(&lib_hash))
            {
                let libs_path = paths.libs.clone();

                futures.push(task::spawn(async move {
                    match download_libs(&lib_name, &lib_path, &lib_url, &lib_hash, &libs_path).await
                    {
                        Ok(lib_info) => Some(lib_info),
                        Err(e) => {
                            println!("{e}");
                            None
                        }
                    }
                }));

                if futures.len() >= 100 {
                    process_futures(&mut futures, &mut downloaded_libs, version_libs.len(), ws)
                        .await;
                }
            }
        }

        process_futures(&mut futures, &mut downloaded_libs, version_libs.len(), ws).await;
    }

    register_libs(downloaded_libs, metacache, paths).await;
    Ok(libs_paths)
}

async fn process_futures(
    futures: &mut FuturesUnordered<async_std::task::JoinHandle<std::option::Option<LibInfo>>>,
    downloaded_libraries: &mut HashSet<LibInfo>,
    max: usize,
    ws: &WebSocketConnection,
) {
    while let Some(result) = futures.next().await {
        if let Some(asset_info) = result {
            let msg: WsMessage = OperationMessage {
                base: BaseMessage {
                    message_id: "todo",
                    operation_id: Some("todo"),
                    request_id: Some("todo"),
                    timestamp: Utc::now(),
                    correlation_id: None,
                },
                data: OperationUpdate::Determinable {
                    stage: STAGE_TYPE,
                    status: ProcessStatus::InProgress,
                    target: Some(ProcessTarget::file(&asset_info.name, FileStatus::Downloaded)),
                    current: downloaded_libraries.len(),
                    total: max,
                }
                .into(),
            }
            .into();

            msg.send(&ws).await.unwrap();

            downloaded_libraries.insert(asset_info);
        }
    }
}

async fn download_libs(
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

async fn register_libs(
    downloaded_libs: HashSet<LibInfo>,
    mut metacache: serde_json::Value,
    paths: &Paths,
) {
    if let Some(libs) = metacache["libraries"].as_array_mut() {
        for item in downloaded_libs.iter() {
            libs.push(json!({
                "hash": item.hash,
                "name": item.name,
                "path": item.path,
            }));
        }
    }

    let mut metacache_file = File::create(&paths.metacache_file).await.unwrap();
    metacache_file
        .write_all(serde_json::to_string_pretty(&metacache).unwrap().as_bytes())
        .await
        .unwrap();
}
