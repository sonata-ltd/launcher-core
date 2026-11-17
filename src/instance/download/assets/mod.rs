use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Instant,
};

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

    #[error("asset {0:?} has no valid SHA-1")]
    InvalidHash(String),

    #[error(transparent)]
    Db(#[from] DbError),
}

#[derive(Debug, Getters, Clone)]
pub struct AssetInfo {
    #[get = "pub"]
    name: String,
    #[get = "pub"]
    hash: String,
    #[get = "pub"]
    url: String,
}

impl AssetInfo {
    fn object_path(&self, objects: &Path) -> PathBuf {
        objects.join(&self.hash[..2]).join(&self.hash)
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
        op.start_stage_determinable(STAGE_TYPE);
        let started = Instant::now();

        let objects_dir = paths.assets().join("objects");
        let wanted = parse::extract_assets(manifest)?;

        let store = DbAssetStore::new(db, objects_dir.clone());
        let outcome = Syncer::default()
            .run(
                wanted,
                |asset| asset.object_path(&objects_dir),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::instance::download::sync::SyncStore;
    use serde_json::json;

    #[test]
    fn invalid_hashes_fail_before_building_paths() {
        for hash in [
            "aé",
            "../file",
            "f",
            "000000000000000000000000000000000000000g",
        ] {
            let manifest = json!({"objects": {"item": {"hash": hash}}});
            assert!(matches!(
                parse::extract_assets(&manifest),
                Err(AssetSyncError::InvalidHash(name)) if name == "item"
            ));
        }
    }

    #[async_std::test]
    async fn asset_cache_checks_contents_even_when_database_already_has_the_asset() {
        let temp = tempfile::tempdir().unwrap();
        let hash = format!("{:x}", <sha1::Sha1 as sha1::Digest>::digest(b"original"));
        let wanted = parse::extract_assets(&json!({"objects": {"item": {"hash": hash}}})).unwrap();
        let path = wanted[0].object_path(temp.path());
        let db = Arc::new(Database::init("sqlite::memory:").await.unwrap());
        let store = DbAssetStore::new(db, temp.path().to_owned());
        store.register(&wanted).await.unwrap();
        assert!(store.cached(&wanted).await.unwrap().is_empty());

        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"original").unwrap();
        assert!(store.cached(&wanted).await.unwrap().contains(&hash));
        std::fs::write(&path, b"modified").unwrap();
        assert!(store.cached(&wanted).await.unwrap().is_empty());
    }
}
