use std::{sync::Arc, time::Instant};

use getset::Getters;
use thiserror::Error;

use crate::{
    data::{
        db::{Database, DbError},
        layout::LauncherPaths,
        registry::operation::{
            handle::OperationHandle,
            message::{
                stage::{OperationStage, StageResult},
                status::Outcome,
            },
        },
    },
    instance::download::{
        assets::storage::DbAssetStore,
        sync::{SyncError, Syncer},
    },
    utils::download::Downloadable,
};

mod parse;
mod storage;

const STAGE_TYPE: OperationStage = OperationStage::DownloadAssets;

#[derive(Debug, Error)]
pub enum AssetSyncError {
    #[error("assets manifest has no `objects` map")]
    ManifestMalformed,

    #[error(transparent)]
    Sync(#[from] SyncError),

    #[error(transparent)]
    Db(#[from] DbError),
}

#[derive(Debug, Getters, Default, Clone)]
pub struct AssetInfo {
    #[get = "pub"]
    name: String,
    #[get = "pub"]
    hash: String,
    #[get = "pub"]
    url: String,
}

impl AssetInfo {
    pub fn new(name: String, hash: String, url: String) -> Self {
        Self { name, hash, url }
    }
}

impl Downloadable for AssetInfo {
    fn get_name(&self) -> &String {
        &self.name
    }

    fn get_hash(&self) -> &String {
        &self.hash
    }

    fn get_url(&self) -> &String {
        &self.url
    }
}

pub struct AssetsOutcome {
    pub total: usize,
    pub downloaded: usize,
}

pub struct AssetsData;

impl AssetsData {
    pub async fn sync(
        manifest: &serde_json::Value,
        paths: &LauncherPaths,
        db: Arc<Database>,
        op: &OperationHandle,
    ) -> Result<AssetsOutcome, AssetSyncError> {
        op.start_stage(STAGE_TYPE);
        let started = Instant::now();

        let objects_dir = paths.assets().join("objects");
        let wanted = parse::extract_assets(manifest)?;

        let store = DbAssetStore::new(db);
        let outcome = Syncer::default()
            .run(
                wanted,
                |asset| {
                    let hash = asset.hash();
                    objects_dir.join(&hash[..2]).join(hash)
                },
                &store,
                op,
                STAGE_TYPE,
            )
            .await?;

        op.complete_stage(StageResult {
            status: Outcome::Completed,
            stage: STAGE_TYPE,
            duration_secs: started.elapsed().as_secs_f64(),
            error: None,
        });

        Ok(AssetsOutcome {
            total: outcome.cached.len() + outcome.downloaded.len(),
            downloaded: outcome.downloaded.len(),
        })
    }
}
