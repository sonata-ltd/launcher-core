use async_std::fs::create_dir_all;
use getset::Getters;
use core::str;
use download::manifest;
use launch::ClientOptions;
use launch::LaunchInfo;
use paths::InstancePaths;
use serde::Deserialize;
use thiserror::Error;

pub mod download;
use download::assets;
use download::libs;
use tide_websockets::WebSocketConnection;

pub mod init;
pub mod launch;
pub mod list;
pub mod paths;
mod websocket;

use crate::utils::instance_manifest::gen_manifest;
use crate::utils::instances_list::add_to_registry;
use crate::websocket::messages::task::Task;
use crate::websocket::messages::task::TaskProgress;
use crate::websocket::messages::task::TaskStatus;
use crate::EndpointRequest;


#[derive(Debug, Deserialize, Clone, Getters)]
pub struct Instance {
    pub name: String,
    pub url: String,
    pub request_id: String,

    #[get = "pub"]
    version: Option<String>,
    manifest: serde_json::Value,
    paths: Option<InstancePaths>,
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

    #[error("Paths is not initialized")]
    PathsNotInitialized,

    #[error("Failed to retrieve instance version")]
    VersionNotAvailable
}

pub type Result<T> = std::result::Result<T, InstanceError>;

impl<'a> Instance {
    pub fn new(name: String, url: String, request_id: String) -> Self {
        // Allocate and init launch_info struct if game_args is passed
        Instance {
            name,
            url,
            version: None,
            manifest: serde_json::Value::Null,
            request_id,
            paths: None,
        }
    }

    pub async fn run(
        self,
        launch_options: Option<ClientOptions>,
        req: &EndpointRequest<'a>,
        ws: &WebSocketConnection,
    ) -> Result<()> {
        let (instance, launch_info) = match Self::init(self, launch_options, req, ws).await {
            Ok(result) => result,
            Err(e) => {
                println!("{e}");
                return Err(e);
            }
        };

        launch::execute::launch_instance(instance.manifest, launch_info).await;
        Ok(())
    }

    async fn register_instance(instance: &Instance) -> Result<()> {
        let paths = instance.paths.as_ref().unwrap();

        match create_dir_all(paths.instance()).await {
            Ok(_) => {
                println!("Initialized instance dir");

                match add_to_registry(&instance, &paths) {
                    Ok(_) => {}
                    Err(e) => return Err(InstanceError::RegistrationFailed(e)),
                };

                match gen_manifest(&instance, &paths) {
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
