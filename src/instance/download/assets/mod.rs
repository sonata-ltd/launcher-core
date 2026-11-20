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
    utils::{cancellation::Cancellation, download::Downloadable, files},
};

pub(crate) mod index;
mod parse;
mod retention;
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

    #[error(transparent)]
    File(#[from] files::FileError),
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
    pub(crate) async fn initialize(db: Arc<Database>, assets: PathBuf) -> Result<(), DbError> {
        let objects = files::regular_directory_child(&assets, Path::new("objects"))?;
        retention::attach(db, objects, Cancellation::new()).await?;
        Self::sweep_imports(assets.clone()).await;
        match retention::collect(assets, std::time::Duration::from_secs(7 * 86400)).await {
            Ok(removed) => tracing::debug!(removed, "asset CAS maintenance completed"),
            Err(error) => tracing::debug!(%error, "asset CAS retention gate deferred"),
        }
        Ok(())
    }

    /// Scratch-only startup maintenance. Published SHA-1 placements, objects,
    /// aliases and SQL metadata are deliberately not collected here.
    pub(crate) async fn sweep_imports(assets: PathBuf) {
        async_std::task::spawn_blocking(move || {
            let result = crate::utils::files::child(&assets, Path::new("objects/.sonata-cas"))
                .map_err(std::io::Error::other)
                .and_then(|root| {
                    crate::utils::object_store::ObjectStore::new(root).sweep_staging()
                });
            match result {
                Ok(removed) if removed > 0 => {
                    tracing::info!(removed, "cleaned up abandoned asset imports")
                }
                Ok(_) => {}
                Err(error) => tracing::warn!(%error, "could not sweep asset import staging"),
            }
        })
        .await;
    }

    pub async fn sync(
        manifest: &serde_json::Value,
        paths: &LauncherPaths,
        db: Arc<Database>,
        op: &OperationHandle,
    ) -> Result<AssetsOutcome, AssetSyncError> {
        Self::sync_with_cancellation(
            parse::extract_assets(manifest)?,
            paths,
            db,
            op,
            &Cancellation::new(),
        )
        .await
    }

    pub(crate) async fn sync_with_cancellation(
        wanted: Vec<AssetInfo>,
        paths: &LauncherPaths,
        db: Arc<Database>,
        op: &OperationHandle,
        cancel: &Cancellation,
    ) -> Result<AssetsOutcome, AssetSyncError> {
        cancel.check().map_err(SyncError::from)?;
        op.start_stage_determinable(STAGE_TYPE);
        let started = Instant::now();

        let objects_dir = files::regular_directory_child(paths.assets(), Path::new("objects"))?;
        let store = DbAssetStore::new(db, objects_dir.clone(), cancel.clone());
        let outcome = Syncer::default()
            .run_with_cancellation(
                wanted,
                |asset| asset.object_path(&objects_dir),
                &store,
                op,
                STAGE_TYPE,
                cancel,
            )
            .await;
        cancel.check().map_err(SyncError::from)?;
        let outcome = outcome?;

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

    #[cfg(any(unix, windows))]
    #[async_std::test]
    async fn initialization_refuses_an_assets_objects_directory_redirect() {
        let fixture = tempfile::tempdir().unwrap();
        let assets = fixture.path().join("assets");
        let separate = fixture.path().join("separate");
        std::fs::create_dir(&assets).unwrap();
        std::fs::create_dir(&separate).unwrap();
        std::fs::write(separate.join("keep"), b"must remain untouched").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&separate, assets.join("objects")).unwrap();
        #[cfg(windows)]
        assert!(std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(assets.join("objects"))
            .arg(&separate)
            .status()
            .unwrap()
            .success());
        let db = Arc::new(Database::init("sqlite::memory:").await.unwrap());
        assert!(AssetsData::initialize(db, assets).await.is_err());
        assert!(!separate.join(".sonata-cas").exists());
        assert_eq!(
            std::fs::read(separate.join("keep")).unwrap(),
            b"must remain untouched"
        );
    }

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
        let store = DbAssetStore::new(db.clone(), temp.path().to_owned(), Cancellation::new());
        sqlx::query("INSERT INTO assets(name,hash,url) VALUES('item',?,'unused')")
            .bind(&hash)
            .execute(&db.pool)
            .await
            .unwrap();
        assert!(store.cached(&wanted).await.unwrap().is_empty());

        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"original").unwrap();
        assert!(store.cached(&wanted).await.unwrap().contains(&hash));
        std::fs::write(&path, b"modified").unwrap();
        // A SQL row still isn't a hit. The new CAS can repair a placement only
        // after verifying the hinted bytes independently of the row/alias.
        assert!(store.cached(&wanted).await.unwrap().contains(&hash));
        assert_eq!(std::fs::read(path).unwrap(), b"original");
    }
}
