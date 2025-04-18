use async_std::{
    fs::{create_dir_all, File},
    io::WriteExt,
    task,
};
use chrono::Utc;
use futures::{stream::FuturesUnordered, StreamExt};
use serde_json::json;
use std::{collections::HashSet, io::Write};
use tide_websockets::WebSocketConnection;

use crate::{
    instance::Paths,
    websocket::messages::WsMessageType,
    websocket::messages::{
        operation::{
            event::OperationUpdate,
            process::{FileStatus, ProcessStatus, ProcessTarget},
            stage::{OperationStage, StageResult, StageStatus},
            OperationMessage,
        },
        BaseMessage, WsMessage,
    },
};

use super::manifest::is_array_exists;


const STAGE_TYPE: OperationStage = OperationStage::DownloadAssets;


pub async fn sync_assets<'a>(
    manifest: &serde_json::Value,
    assets_path: &'a str,
    ws: &WebSocketConnection,
    paths: &Paths,
) {
    let msg: WsMessage = OperationMessage {
        base: BaseMessage {
            message_id: "todo",
            operation_id: Some("todo"),
            request_id: Some("todo"),
            timestamp: Utc::now(),
            correlation_id: None,
        },
        data: OperationUpdate::Determinable {
            stage: OperationStage::DownloadAssets,
            status: ProcessStatus::Started,
            target: None,
            current: 0,
            total: 0,
        }
        .into(),
    }
    .into();

    msg.send(&ws).await.unwrap();

    extract_manifest_assets(manifest, assets_path, ws, paths).await;
    println!("Asset extraction completed");

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
            duration_secs: 0.0, // TODO
            error: None,
        })
        .into(),
    }
    .into();

    msg.send(&ws).await.unwrap();
}

#[derive(Eq, PartialEq, Debug, Hash)]
struct AssetInfo {
    name: String,
    hash: String,
}

async fn extract_manifest_assets<'a>(
    manifest: &'a serde_json::Value,
    assets_path: &str,
    ws: &WebSocketConnection,
    paths: &Paths,
) {
    let base_url = "https://resources.download.minecraft.net/";
    let metacache_file = std::fs::File::open(&paths.metacache_file).unwrap();
    let mut metacache: serde_json::Value = serde_json::from_reader(&metacache_file).unwrap();
    let mut downloaded_assets: HashSet<AssetInfo> = HashSet::new();

    if !is_array_exists(&metacache, "assets") {
        if let Some(metacache_object) = metacache.as_object_mut() {
            metacache_object.insert("assets".to_string(), json!([]));
            let mut metacache_file = std::fs::File::create(&paths.metacache_file).unwrap();
            metacache_file
                .write_all(serde_json::to_string_pretty(&metacache).unwrap().as_bytes())
                .unwrap();
        }
    }

    if let Some(objects) = manifest["objects"].as_object() {
        if let Some(assets) = metacache["assets"].as_array() {
            let mut futures = FuturesUnordered::new();

            println!("Checking for assets...");

            for (k, v) in objects {
                if !assets.iter().any(|asset| {
                    asset["name"].as_str() == Some(k)
                        && asset["hash"].as_str() == v["hash"].as_str()
                }) {
                    let base_url = base_url.to_string();
                    let hash = v["hash"].as_str().unwrap().to_string();
                    let name = k.to_string();
                    let assets_path = assets_path.to_string();

                    futures.push(task::spawn(async move {
                        println!("Downloading asset '{}'", name);
                        match download_asset(&base_url, &hash, &name, &assets_path).await {
                            Ok(asset_info) => Some(asset_info),
                            Err(e) => {
                                println!("{e}");
                                None
                            }
                        }
                    }));

                    if futures.len() >= 100 {
                        process_futures(&mut futures, &mut downloaded_assets, objects.len(), ws)
                            .await;
                    }
                }
            }

            process_futures(&mut futures, &mut downloaded_assets, objects.len(), ws).await;
        }
    }

    register_assets(downloaded_assets, metacache, paths).await;
}

async fn process_futures(
    futures: &mut FuturesUnordered<async_std::task::JoinHandle<std::option::Option<AssetInfo>>>,
    downloaded_assets: &mut HashSet<AssetInfo>,
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
                    current: downloaded_assets.len(),
                    total: max,
                }
                .into(),
            }
            .into();

            msg.send(&ws).await.unwrap();

            downloaded_assets.insert(asset_info);
        }
    }
}

async fn download_asset(
    base_url: &str,
    asset_hash: &str,
    asset_name: &str,
    path: &str,
) -> Result<AssetInfo, String> {
    let asset_url_data = construct_asset_url(&base_url, &asset_hash.to_string());
    let (full_url, hash_part, hash) = asset_url_data;

    match create_dir_all(format!("{}/{}", path, hash_part)).await {
        Ok(_) => {}
        Err(e) => {
            println!("Failed to create dir for asset: {e}");
            return Err(e.to_string());
        }
    }

    match surf::get(&full_url).await {
        Ok(mut response) => {
            let mut file = File::create(format!("{}/{}/{}", path, hash_part, hash))
                .await
                .unwrap();
            async_std::io::copy(&mut response, &mut file).await.unwrap();

            Ok(AssetInfo {
                name: asset_name.to_string(),
                hash: asset_hash.to_string(),
            })
        }
        Err(e) => {
            println!("{e}");
            Err(e.to_string())
        }
    }
}

async fn register_assets(
    downloaded_assets: HashSet<AssetInfo>,
    mut metacache: serde_json::Value,
    paths: &Paths,
) {
    if let Some(assets) = metacache["assets"].as_array_mut() {
        for item in downloaded_assets.iter() {
            assets.push(json!({
                "name": item.name,
                "hash": item.hash,
            }));
        }
    } else {
        println!("Failed to find \"assets\" array in metacache file");
        return;
    }

    let mut metacache_file = File::create(&paths.metacache_file).await.unwrap();
    metacache_file
        .write_all(serde_json::to_string_pretty(&metacache).unwrap().as_bytes())
        .await
        .unwrap();
}

fn construct_asset_url(base_url: &str, hash: &String) -> (String, String, String) {
    let hash_part = &hash[0..2].to_string();

    (
        format!("{}{}/{}", base_url, hash_part, hash),
        hash_part.to_string(),
        hash.to_string(),
    )
}
