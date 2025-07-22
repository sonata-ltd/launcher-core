use async_std::fs::create_dir_all;
use chrono::Utc;
use download::manifest;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::PathBuf;
use thiserror::Error;

pub mod download;
use download::assets;
use download::libs;
use tide_websockets::WebSocketConnection;

pub mod init;
pub mod launch;
pub mod list;

use crate::utils::instance_manifest::gen_manifest;
use crate::utils::instances_list::add_to_registry;
use crate::websocket::messages::task::Task;
use crate::websocket::messages::task::TaskProgress;
use crate::websocket::messages::task::TaskStatus;
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

pub struct Paths<'a> {
    pub root: &'a PathBuf,
    pub libs: PathBuf,
    pub assets: PathBuf,
    pub instance: PathBuf,
    pub instance_manifest_file: PathBuf,
    pub instances_list_file: PathBuf,
    pub headers: PathBuf,
    pub meta: PathBuf,
    pub version_manifest_file: Option<PathBuf>,
    pub metacache_file: PathBuf,
}

pub struct LaunchInfo<'a> {
    version_manifest: serde_json::Value,
    paths: Paths<'a>,
}

pub struct InstanceInfo {
    pub version: String,
}

#[derive(Debug, Deserialize, Clone)]
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
    HomeDirNotFound,
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

    pub async fn run(mut self, req: &EndpointRequest<'a>, ws: &WebSocketConnection) -> Result<()> {
        // let LaunchInfo { version_manifest, paths } =
        //     match Self::init(&mut self, req, ws).await {
        //         Ok(launch_info) => launch_info,
        //         Err(e) => return Err(InstanceError::RunFailed())
        //     };

        // println!("{}", version_manifest);
        // // launch::launch_instance(version_manifest, &self.info.unwrap(), &paths).await;

        // Ok(())

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
                    return Err(InstanceError::RunFailed(format!(
                        "info hashmap is not provided"
                    )));
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
        paths: &Paths<'a>,
        instance_info: &InstanceInfo,
    ) -> Result<()> {
        println!("{:?}", &paths.instance);
        match create_dir_all(&paths.instance).await {
            Ok(_) => {
                println!("Initialized instance dir");

                match add_to_registry(&instance.name, &paths) {
                    Ok(_) => {}
                    Err(e) => return Err(InstanceError::RegistrationFailed(e)),
                };

                match gen_manifest(&instance, &paths, &instance_info) {
                    Ok(_) => {}
                    Err(e) => return Err(InstanceError::ManifestGenerationFailed(e)),
                };

                Ok(())
            }
            Err(e) => {
                return Err(InstanceError::DirCreationFailed(e.to_string()));
            }
        }
    }
}

// Return Libs path, Assets path, Instances path
fn get_required_paths<'a>(instance_name: &String, launcher_root: &'a PathBuf) -> Result<Paths<'a>> {
    Ok(Paths {
        libs: launcher_root.join("libraries"),
        assets: launcher_root.join("assets"),
        instance: launcher_root.join("instances").join(instance_name),
        instance_manifest_file: launcher_root.join("headers").join(instance_name),
        instances_list_file: launcher_root.join("headers").join("main.json"),
        headers: launcher_root.join("headers"),
        meta: launcher_root.join("meta"),
        version_manifest_file: None,
        metacache_file: launcher_root.join("metacache.json"),
        root: launcher_root,
    })
}
