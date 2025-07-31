use std::{collections::HashSet, io::Write, path::Path, sync::Arc};

use async_std::{
    fs::{create_dir_all, File},
    io::WriteExt,
    stream::StreamExt,
    task,
};
use futures::stream::FuturesUnordered;
use getset::Getters;
use serde_json::json;

use crate::{
    instance::{
        download::manifest::is_array_exists,
        websocket::{OperationWsExt, OperationWsMessageLocked},
    },
    websocket::messages::operation::{
        process::{FileStatus, ProcessTarget},
        stage::{OperationStage, StageStatus},
    },
};

mod parse;
mod download;
mod register;

const STAGE_TYPE: OperationStage = OperationStage::DownloadAssets;

pub struct AssetsData<'a> {
    manifest: &'a serde_json::Value,
    assets_path: String,
    metacache_file_path: String,
    ws_status: OperationWsMessageLocked<'a>,
}

#[derive(Eq, PartialEq, Debug, Hash, Getters)]
pub struct AssetInfo {
    name: String,
    hash: String,
}

impl<'a> AssetsData<'a> {
    pub async fn sync_assets<T>(
        manifest: &'a serde_json::Value,
        assets_path: T,
        metacache_file_path: T,
        ws_status: OperationWsMessageLocked<'a>,
    ) where
        T: AsRef<Path>,
    {
        ws_status
            .clone()
            .start_stage_determinable(STAGE_TYPE, None, 0, 0)
            .await;

        let assets_data = AssetsData {
            manifest,
            assets_path: assets_path.as_ref().display().to_string(),
            metacache_file_path: metacache_file_path.as_ref().display().to_string(),
            ws_status: ws_status.clone(),
        };

        Self::extract_manifest_assets(&assets_data).await;

        ws_status
            .complete_stage(StageStatus::Completed, STAGE_TYPE, 0.0, None)
            .await;
    }
}
