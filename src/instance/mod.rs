use async_std::fs::create_dir_all;
use core::str;
use getset::Getters;
use launch::ClientOptions;
use launch::LaunchInfo;
use paths::InstancePaths;
use serde::Deserialize;
use thiserror::Error;

pub mod download;
use tide_websockets::WebSocketConnection;

pub mod init;
pub mod launch;
pub mod list;
pub mod paths;
mod websocket;

use crate::manifest::instance::gen_manifest;
use crate::manifest::instance::uuid::UuidData;
use crate::utils::instances_list::add_to_registry;
use crate::websocket::messages::task::Task;
use crate::websocket::messages::task::TaskProgress;
use crate::websocket::messages::task::TaskStatus;
use crate::EndpointRequest;

#[derive(Deserialize)]
pub struct InitData {
    pub name: String,
    pub url: String,
    pub request_id: String,
}

#[derive(Deserialize, Debug)]
pub struct RunData {
    name: String,
    url: String,
    request_id: String,
    launch_options: Option<ClientOptions>,
}

#[derive(Debug, Deserialize, Clone, Getters)]
pub struct Instance {
    pub name: String,
    pub url: String,
    pub request_id: String,

    #[get = "pub"]
    version_id: String,
    version_manifest: serde_json::Value,
    paths: InstancePaths,
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
    VersionNotAvailable,
}

pub type Result<T> = std::result::Result<T, InstanceError>;

impl<'a> Instance {
    pub async fn run(
        run_data: RunData,
        req: &EndpointRequest<'a>,
        ws: &WebSocketConnection,
    ) -> Result<()> {
        let init_data = InitData {
            name: run_data.name,
            url: run_data.url,
            request_id: run_data.request_id,
        };

        let (instance, launch_info) =
            match Self::init(init_data, run_data.launch_options, req, ws).await {
                Ok(result) => result,
                Err(e) => {
                    println!("{e}");
                    return Err(e);
                }
            };

        launch::execute::launch_instance(instance.version_manifest, launch_info).await;
        Ok(())
    }

    async fn register_instance(instance: &Instance) -> Result<()> {
        let paths = &instance.paths;

        let uuid = UuidData::new()
            .add_name(&instance.name)
            .add_version(instance.version_id())
            .gen();

        match create_dir_all(paths.instance()).await {
            Ok(_) => {
                match add_to_registry(&instance, &paths, &uuid) {
                    Ok(_) => {}
                    Err(e) => return Err(InstanceError::RegistrationFailed(e)),
                };

                match gen_manifest(&instance, &paths, &uuid).await {
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
