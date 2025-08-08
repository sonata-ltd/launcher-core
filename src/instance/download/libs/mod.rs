use std::{env::consts::OS, sync::Arc};

use crate::{
    instance::{
        paths::InstancePaths,
        websocket::{OperationWsExt, OperationWsMessageLocked},
    },
    websocket::messages::operation::stage::{OperationStage, StageStatus},
};

mod download;
mod parse;
mod register;

pub struct LibsData<'a> {
    manifest: &'a serde_json::Value,
    paths: &'a InstancePaths,
    ws_status: OperationWsMessageLocked<'a>,
    current_os: &'a str,
}

#[derive(Eq, Hash, PartialEq, Debug, Clone)]
pub struct LibInfo {
    hash: String,
    name: String,
    path: String,
}

const STAGE_TYPE: OperationStage = OperationStage::DownloadLibs;

impl<'a> LibsData<'a> {
    pub async fn sync_libs(
        manifest: &'a serde_json::Value,
        paths: &'a InstancePaths,
        ws_status: OperationWsMessageLocked<'a>,
    ) -> Result<Vec<String>, String> {
        // Sync status through WebSocket
        ws_status
            .clone()
            .start_stage_determinable(STAGE_TYPE, None, 0, 0)
            .await;

        let current_os_supported = match OS {
            "linux" => "linux",
            "macos" => "osx",
            "windows" => "windows",
            _ => return Err("Unsupported OS".into())
        };

        let libs_data = LibsData {
            manifest,
            paths,
            ws_status: ws_status.clone(),
            current_os: current_os_supported,
        };

        let done_paths = match Self::extract_manifest_libs(libs_data).await {
            Ok(paths) => paths,
            Err(e) => return Err(e),
        };

        ws_status
            .complete_stage(StageStatus::Completed, STAGE_TYPE, 0.0, None)
            .await;

        Ok(done_paths)
    }
}
