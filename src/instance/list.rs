use chrono::Utc;
use std::fs::OpenOptions;

use async_std::path::Path;
use tide_websockets::WebSocketConnection;

use crate::{
    utils::instances_list,
    websocket::messages::{
        operation::{
            event::{OperationFinish, OperationStart, OperationStatus, OperationUpdate},
            process::{ProcessStatus, ProcessTarget},
            stage::OperationStage,
            OperationMessage,
        },
        scan::ScanInfo,
        BaseMessage, WsMessage, WsMessageType,
    },
};

const STAGE_TYPE: OperationStage = OperationStage::ScanInstances;

pub struct List {
    manifest_location: String,
}

impl List {
    pub fn new(manifest_location: String) -> List {
        List { manifest_location }
    }

    fn extract_instance_data<'a>(config: &str) -> Option<ScanInfo> {
        let instance_manifest_file = OpenOptions::new()
            .read(true)
            .write(false)
            .create(false)
            .open(config)
            .unwrap();

        let instance_manifest: serde_json::Value =
            match serde_json::from_reader(&instance_manifest_file) {
                Ok(value) => value,
                Err(_) => {
                    println!("can't read manifest");
                    return None;
                }
            };

        if let Some(general) = instance_manifest.get("general").and_then(|v| v.as_object()) {
            let version = general.get("version").and_then(|v| v.as_str())?;
            println!("ver: {}", version);
            let loader = general.get("loader").and_then(|v| v.as_str())?;
            println!("loa: {}", loader);

            if let Some(overview) = instance_manifest
                .get("overview")
                .and_then(|v| v.as_object())
            {
                let name = overview.get("name").and_then(|v| v.as_str())?;
                println!("{}", name);

                Some(ScanInfo {
                    name: name.to_string(),
                    version: version.to_string(),
                    loader: loader.to_string(),
                })
            } else {
                println!("Overview name is not found");
                None
            }
        } else {
            println!("general not found");
            None
        }
    }

    pub async fn start_paths_checking(&self, ws: &WebSocketConnection) -> Result<String, String> {
        let main_manifest_file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .open(&self.manifest_location)
            .unwrap();

        let main_manifest = match serde_json::from_reader(&main_manifest_file) {
            Ok(value) => value,
            Err(_) => match instances_list::recreate(&self.manifest_location) {
                Ok((_file, value)) => {
                    println!("File not found, recreated");
                    value
                }
                Err(e) => {
                    println!("Failed to recreate instances main manifest file: {}", e);
                    return Err(format!(
                        "Failed to recreate instances main manifest file: {e}"
                    ));
                }
            },
        };

        if let Some(instances) = main_manifest["instances"].as_array() {
            // HashMap contains:
            // instance_manifest_path, instance_manifest_exist | instance_path, instance_exist
            // let mut instances_exists: HashMap<(&str, bool), (&str, bool)> = HashMap::new();

            let msg: WsMessage = OperationMessage {
                base: BaseMessage {
                    message_id: "asd".to_string(),
                    operation_id: Some("asd".to_string()),
                    request_id: Some("asd".to_string()),
                    timestamp: Utc::now(),
                    correlation_id: None,
                },
                data: OperationStart {
                    stages: vec![STAGE_TYPE],
                }
                .into(),
            }
            .into();

            msg.send(&ws).await.unwrap();
            println!("send");

            for (i, item) in instances.iter().enumerate() {
                if let (Some(manifest_path), Some(instance_path)) =
                    (item["config"].as_str(), item["folder"].as_str())
                {
                    let manifest_exist = Path::new(manifest_path).exists().await;
                    let instance_exist = Path::new(instance_path).exists().await;

                    // Get ScanInfo data if the instance manifest exists and is not corrupted
                    let scan_info = Self::extract_instance_data(&manifest_path);
                    println!("{:#?}", scan_info);

                    let msg: WsMessage = OperationMessage {
                        base: BaseMessage {
                            message_id: "asd".to_string(),
                            operation_id: Some("asd".to_string()),
                            request_id: Some("asd".to_string()),
                            timestamp: Utc::now(),
                            correlation_id: None,
                        },
                        data: OperationUpdate::Determinable {
                            stage: STAGE_TYPE,
                            status: ProcessStatus::InProgress,
                            target: Some(ProcessTarget::instance(
                                manifest_path.to_string(),
                                manifest_exist,
                                instance_path.to_string(),
                                instance_exist,
                                scan_info,
                            )),
                            current: i + 1,
                            total: instances.len(),
                        }
                        .into(),
                    }
                    .into();

                    // let msg: WsMessage = ScanMessage {
                    //     base: BaseMessage {
                    //         message_id: "asd",
                    //         operation_id: Some("asd"),
                    //         request_id: Some("asd"),
                    //         timestamp: Utc::now(),
                    //         correlation_id: None,
                    //     },
                    //     data: ScanData {
                    //         integrity: ScanIntegrity {
                    //             manifest_path: config,
                    //             manifest_exist,
                    //             instance_path: folder,
                    //             instance_exist,
                    //         },
                    //         info: scan_info,
                    //     }
                    //     .into(),
                    // }
                    // .into();

                    msg.send(&ws).await.unwrap();
                }
            }

            // let msg = InfoMessage {
            //     base: BaseMessage {
            //         message_id: format!("scan_complete"),
            //         timestamp: format!("Currect Date"),
            //         request_id: None,
            //     },
            //     message: format!("Scan Complete"),
            // };

            // if let Err(e) = send_ws_msg(ws, json!(msg)).await {
            //     println!("Failed to send update info, {e}");
            // }

            let msg: WsMessage = OperationMessage {
                base: BaseMessage {
                    message_id: "asd".to_string(),
                    operation_id: Some("asd".to_string()),
                    request_id: Some("asd".to_string()),
                    timestamp: Utc::now(),
                    correlation_id: None,
                },
                data: OperationFinish {
                    status: OperationStatus::Completed,
                }
                .into(),
            }
            .into();

            msg.send(&ws).await.unwrap();

            return Ok(format!("{:#?}", instances));
        } else {
            println!("Not found");
            return Err(format!("Not found"));
        }
    }
}
