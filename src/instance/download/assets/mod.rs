use std::{hash::{Hash, Hasher}, path::Path};

use futures::stream::FuturesUnordered;
use getset::Getters;
use thiserror::Error;

use crate::{
    data::db::Database, instance::
        websocket::{OperationWsExt, OperationWsMessageLocked}, utils::download::Downloadable, websocket::messages::operation::
        stage::{OperationStage, StageStatus}

};

mod parse;
mod download;
mod register;

const STAGE_TYPE: OperationStage = OperationStage::DownloadAssets;

pub struct AssetsData<'a> {
    manifest: &'a serde_json::Value,
    assets_path: String,
    ws_status: OperationWsMessageLocked<'a>,
    db: &'a Database
}

#[derive(Debug, Getters, Default)]
pub struct AssetInfo {
    #[get = "pub"]
    name: String,
    hash: String,
    #[get = "pub"]
    url: String
}

impl Downloadable for AssetInfo {
    fn get_name(&self) -> &String {
        self.name()
    }

    fn get_hash(&self) -> &String {
        self.get_hash()
    }

    fn get_url(&self) -> &String {
        self.url()
    }
}

#[derive(Debug, Error)]
pub enum AssetSyncError {
    #[error("Failed to register a new asset to DB: {0}")]
    RegisterFailed(String)
}


impl<'a> AssetsData<'a> {
    pub async fn sync_assets<T>(
        manifest: &'a serde_json::Value,
        assets_path: T,
        ws_status: OperationWsMessageLocked<'a>,
        db: &'a Database
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
            ws_status: ws_status.clone(),
            db
        };

        match Self::extract_manifest_assets(&assets_data).await {
            Ok(_) => (),
            Err(e) => println!("{e}")
        }

        ws_status
            .complete_stage(StageStatus::Completed, STAGE_TYPE, 0.0, None)
            .await;
    }
}

impl PartialEq for AssetInfo {
    fn eq(&self, other: &Self) -> bool {
        self.hash == other.hash
    }
}

impl Eq for AssetInfo {}

impl Hash for AssetInfo {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.hash.hash(state);
    }
}

impl AssetInfo {
    pub fn with_hash(hash: &str) -> Self {
        let mut asset = AssetInfo::default();
        asset.hash = hash.to_string();
        asset
    }

    pub fn get_hash(&self) -> &String {
        &self.hash
    }
}
