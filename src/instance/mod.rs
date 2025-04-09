use async_std::fs::create_dir_all;
use chrono::Utc;
use download::manifest;
use home::home_dir;
use serde::Deserialize;
use thiserror::Error;
use std::collections::HashMap;

pub mod download;
use download::assets;
use download::libs;
use tide_websockets::WebSocketConnection;

pub mod launch;
pub mod list;

use crate::data::task::Task;
use crate::utils::instance_manifest::gen_manifest;
use crate::utils::instances_list::add_to_registry;
use crate::websocket::messages::WsMessageType;
use crate::websocket::messages::{
    operation::{
        event::{OperationStart, OperationUpdate},
        stage::{OperationStage, StageResult, StageStatus},
        OperationMessage,
    },
    BaseMessage, WsMessage,
};
use crate::EndpointRequest;


pub struct Paths {
    pub root: String,
    pub libs: String,
    pub assets: String,
    pub instance: String,
    pub instance_manifest_file: String,
    pub instances_list_file: String,
    pub headers: String,
    pub meta: String,
    pub version_manifest_file: Option<String>,
    pub metacache_file: String,
}

pub struct LaunchInfo {
    version_manifest: serde_json::Value,
    paths: Paths,
}

pub struct InstanceInfo {
    pub version: String,
}

#[derive(Debug, Deserialize)]
pub struct Instance {
    pub name: String,
    pub url: String,
    pub info: Option<HashMap<String, String>>,
    pub request_id: String,
}

#[derive(Error, Debug)]
pub enum InstanceError {
    #[error("Failed to create new instance: {0}")]
    CreationFailed(String),

    #[error("Failed to run instance: {0}")]
    RunFailed(String),

    #[error("Failed to register instance: {0}")]
    RegistrationFailed(String),

    #[error("Failed to generate manifest for instance: {0}")]
    ManifestGenerationFailed(String),

    #[error("Failed to create instance directory: {0}")]
    DirCreationFailed(String),

    #[error("Failed to get home direcory")]
    HomeDirNotFound
}

pub type Result<T> = std::result::Result<T, InstanceError>;

impl<'a> Instance {
    pub fn new(
        name: String,
        url: String,
        info: Option<HashMap<String, String>>,
        request_id: String,
    ) -> Instance {
        Instance {
            name,
            url,
            info,
            request_id,
        }
    }

    pub async fn init(
        &mut self,
        req: &EndpointRequest<'a>,
        ws: &WebSocketConnection
    ) -> Result<LaunchInfo> {
        // Init task
        let global_app_state = req.state();
        let task_handle = match global_app_state.add_task(
            Task::new_shared("download instance")
        ).await {
            Ok(handle) => handle,
            Err(e) => return Err(InstanceError::CreationFailed(e.to_string()))
        };

        global_app_state.update_task(task_handle.id, |t| {
            t.name = "instance is downloaded";
        }).await.unwrap();


        // Get default paths
        let mut paths = match get_required_paths(&self.name) {
            Ok(paths) => paths,
            Err(e) => return Err(e),
        };

        // Update launch arguments if info is not `None`
        self.update_info("${game_directory}", paths.instance.to_string());
        self.update_info("${assets_root}", paths.assets.to_string());
        self.update_info("${user_properties}", "{}".to_string());

        let msg: WsMessage = OperationMessage {
            base: BaseMessage {
                message_id: "todo",
                operation_id: Some("todo"),
                request_id: Some("todo"),
                timestamp: Utc::now(),
                correlation_id: None,
            },
            data: OperationStart {
                stages: vec![
                    OperationStage::FetchManifest,
                    OperationStage::DownloadLibs,
                    OperationStage::DownloadAssets,
                ],
            }
            .into(),
        }
        .into();

        msg.send(&ws).await.unwrap();

        // Get Minecraft version manifest - Stage 1
        // TODO: Find already downloaded manifest and redownload
        // it if outdated
        let version_manifest = match manifest::download_manifest(&self.url, &paths.meta).await {
            Ok((data, path_to_manifest)) => {
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
                        stage: OperationStage::FetchManifest,
                        duration_secs: 0.0, // TODO
                        error: None,
                    })
                    .into(),
                }
                .into();

                msg.send(&ws).await.unwrap();

                // Update info in Paths structure for instance manifest generation
                paths.version_manifest_file = path_to_manifest;

                data
            }
            Err(e) => return Err(
                InstanceError::CreationFailed(
                    format!("Failed to download version manifest: {}", e)
                )
            )
        };

        // Sync & download all libs needed by this version - Stage 2
        match libs::sync_libs(&version_manifest, &paths, &ws).await {
            Ok((dir, paths)) => {
                let mut paths_string = String::new();

                for path in paths.iter() {
                    paths_string.push_str(&path);
                }

                // Update launch args if info is not None
                self.update_info("${libs_directory}", dir.to_string());
                self.update_info("${classpath_libs_directories}", paths_string);
            }
            Err(e) => return Err(
                InstanceError::CreationFailed(
                    format!("Failed to download and register libs: {e}")
                )
            )
        };

        // Get version assets manifest
        let assets_manifest_location = paths.assets.to_owned() + "/indexes";
        println!("{}", version_manifest);
        let assets_manifest =
            match manifest::get_assets_manifest(&version_manifest, &assets_manifest_location).await
            {
                Ok((data, id)) => {
                    self.update_info("${assets_index_name}", id.to_string());
                    data
                }
                Err(e) => return Err(
                    InstanceError::CreationFailed(
                        format!("Failed to download assets manifest: {}", e)
                    )
                )
            };

        // Sync & download all assets needed by this version - Stage 3
        let assets_objects_location = paths.assets.to_owned() + "/objects";
        assets::sync_assets(&assets_manifest, &assets_objects_location, ws, &paths).await;

        // Initialize instance directory
        let instance_version = match version_manifest["id"].as_str() {
            Some(data) => InstanceInfo {
                version: data.to_string(),
            },
            None => return Err(
                InstanceError::CreationFailed(
                    "Failed to determine version".to_string()
                )
            )
        };

        match Self::register_instance(&self, &paths, &instance_version).await {
            Ok(_) => {}
            Err(e) => return Err(
                InstanceError::CreationFailed(
                    format!("Failed to initialize instance directory: {}", e)
                )
            )
        };

        return Ok(LaunchInfo {
            version_manifest,
            paths,
        });
    }

    pub async fn run(
        mut self,
        req: &EndpointRequest<'a>,
        ws: &WebSocketConnection
    ) -> Result<()> {
        match Self::init(&mut self, req, ws).await {
            Ok(launch_info) => {
                if let Some(info) = self.info {
                    let LaunchInfo {
                        version_manifest,
                        paths,
                    } = launch_info;

                    println!("{}", version_manifest);

                    launch::launch_instance(version_manifest, &info, &paths).await;

                    return Ok(());
                } else {
                    return Err(
                        InstanceError::RunFailed(
                            format!("info hashmap is not provided")
                        )
                    )
                }
            }
            Err(e) => {
                println!("{e}");
                return Err(e);
            }
        }
    }

    fn update_info(&mut self, k: &'a str, v: String) {
        if let Some(info_map) = &mut self.info {
            info_map.insert(k.to_string(), v);
        }
    }

    async fn register_instance(
        instance: &Instance,
        paths: &Paths,
        instance_info: &InstanceInfo,
    ) -> Result<()> {
        match create_dir_all(&paths.instance).await {
            Ok(_) => {
                println!("Initialized instance dir");

                match add_to_registry(&instance.name, &paths) {
                    Ok(_) => {}
                    Err(e) => return Err(
                        InstanceError::RegistrationFailed(e)
                    )
                };

                match gen_manifest(&instance, &paths, &instance_info) {
                    Ok(_) => {}
                    Err(e) => return Err(
                        InstanceError::ManifestGenerationFailed(e)
                    )
                };

                Ok(())
            }
            Err(e) => {
                return Err(
                    InstanceError::DirCreationFailed(e.to_string())
                );
            }
        }
    }
}

// Return Libs path, Assets path, Instances path
fn get_required_paths(instance_name: &String) -> Result<Paths> {
    let root = match home_dir() {
        Some(path) => format!("{}/.sonata", path.display()),
        None => return Err(InstanceError::HomeDirNotFound),
    };

    Ok(Paths {
        libs: format!("{}/libraries", root),
        assets: format!("{}/assets", root),
        instance: format!("{}/instances/{}", root, instance_name),
        instance_manifest_file: format!("{}/headers/{}.json", root, instance_name),
        instances_list_file: format!("{}/headers/main.json", root),
        headers: format!("{}/headers", root),
        meta: format!("{}/meta", root),
        version_manifest_file: None,
        metacache_file: format!("{}/metacache.json", root),
        root,
    })
}
