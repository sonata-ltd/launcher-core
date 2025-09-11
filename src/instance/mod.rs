use async_std::fs::create_dir_all;
use chrono::Utc;
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
pub mod options;
pub mod paths;
mod websocket;

use crate::data::db::DBError;
use crate::data::db::Database;
use crate::instance::options::pages::Page;
use crate::instance::options::ChangeRequestBuilder;
use crate::instance::options::Options;
use crate::utils::db::register_instance;
use crate::websocket::messages::option::OptionUpdateMessage;
use crate::websocket::messages::task::Task;
use crate::websocket::messages::task::TaskProgress;
use crate::websocket::messages::task::TaskStatus;
use crate::websocket::messages::BaseMessage;
use crate::websocket::messages::WsMessage;
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
    #[get = "pub"]
    paths: InstancePaths,
}

#[derive(Error, Debug)]
pub enum InstanceError {
    #[error("Failed to create new instance: {0}")]
    CreationFailed(String),

    #[error("Instance not found: {0}")]
    InstanceNotFound(String),

    #[error("Failed to run instance: {0}")]
    RunFailed(String),

    #[error("Instance id is wrong: {0}")]
    WrongId(String),

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

    #[error("Failed to read option: {0}")]
    OptionNotAvailable(String),

    #[error("Wrong options page is present: {0}")]
    OptionsPageWrong(String),

    #[error("Failed to convert into JSON: {0}")]
    JSONConstructionFailed(#[from] serde_json::Error),

    #[error(transparent)]
    DB(#[from] DBError),

    #[error("Function is not implemented yet")]
    NotImplemented
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
            match Self::init(init_data, false, run_data.launch_options, req, ws).await {
                Ok(result) => result,
                Err(e) => {
                    println!("{e}");
                    return Err(e);
                }
            };

        launch::execute::launch_instance(instance.version_manifest, launch_info).await;
        Ok(())
    }

    pub async fn get_page(req: &EndpointRequest<'a>, id: i64, page: Page) -> Result<serde_json::Value> {
        let db = &req.state().static_data.db;

        let page_data = Options::retrieve(&db, id, page).await?;
        let msg: WsMessage = WsMessage::Option(OptionUpdateMessage {
            base: BaseMessage {
                message_id: "asdasd".into(),
                operation_id: Some("".into()),
                request_id: Some("".into()),
                timestamp: Utc::now(),
                correlation_id: None
            },
            option: page_data.into()
        });

        match serde_json::to_value(msg) {
            Ok(value) => Ok(value),
            Err(e) => Err(InstanceError::JSONConstructionFailed(e))
        }
    }

    pub async fn change_field(req: &EndpointRequest<'a>, request: ChangeRequestBuilder) -> Result<()> {
        let db = &req.state().static_data.db;
        let builded_request = request.build()?;

        Options::change(&db, builded_request).await?;

        Ok(())
    }

    async fn register(db: &Database, instance: &Instance) -> Result<i64> {
        let paths = &instance.paths;

        match create_dir_all(paths.instance()).await {
            Ok(_) => {
                let result = register_instance(db, instance).await?;
                Ok(result)
            }
            Err(e) => {
                return Err(InstanceError::DirCreationFailed(e.to_string()));
            }
        }
    }
}
