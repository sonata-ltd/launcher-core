use std::{collections::HashMap, path::PathBuf, sync::Arc, time::Instant};

use getset::Getters;
use serde::Deserialize;
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
    instance::{
        download::{
            libs::storage::DbLibStore,
            sync::{SyncError, Syncer},
        },
        model::InstanceId,
    },
    utils::{download::Downloadable, files, maven},
    version::provider::MetaProvider,
};

mod parse;
mod storage;

#[derive(Error, Debug)]
pub enum LibsSyncError {
    #[error("OS is not supported")]
    OsNotAvailable,

    #[error("CPU architecture is not supported")]
    ArchNotAvailable,

    #[error("failed to parse manifest: {0}")]
    ManifestMalformed(String),

    #[error(transparent)]
    Sync(#[from] SyncError),

    #[error(transparent)]
    Db(#[from] DbError),
}

#[derive(Debug)]
pub struct LibsOutcome {
    pub classpath: Vec<PathBuf>,
    pub natives: Vec<PathBuf>,
}

pub struct LibsData<'a, 'b> {
    manifest: &'b serde_json::Value,
    paths: &'a LauncherPaths,
    provider: MetaProvider,
    current_os: &'a str,
}

#[derive(Eq, Hash, PartialEq, Debug, Clone, Deserialize, sqlx::FromRow, Getters)]
pub struct LibInfo {
    #[get = "pub"]
    hash: String,
    #[get = "pub"]
    name: String,
    #[get = "pub"]
    path: String,
    #[get = "pub"]
    url: String,
    native: bool,
}

impl Downloadable for LibInfo {
    fn get_name(&self) -> &String {
        self.name()
    }

    fn get_hash(&self) -> &String {
        self.hash()
    }

    fn get_url(&self) -> &String {
        self.url()
    }
}

impl LibInfo {
    pub fn new(
        name: String,
        hash: String,
        path: PathBuf,
        url: String,
        native: bool,
    ) -> Result<Self, LibsSyncError> {
        if !files::hash_valid(&hash, 40) {
            return Err(LibsSyncError::ManifestMalformed("invalid SHA-1".into()));
        }

        let path = path
            .into_os_string()
            .into_string()
            .map_err(|_| LibsSyncError::ManifestMalformed("non-Unicode library path".into()))?;

        Ok(Self {
            hash,
            name,
            path,
            url,
            native,
        })
    }

    pub fn is_native(&self) -> bool {
        self.native
    }
}

pub async fn linked_libs(id: InstanceId, db: Arc<Database>) -> Result<Vec<LibInfo>, DbError> {
    DbLibStore::new(db).for_instance(id).await
}

const STAGE_TYPE: OperationStage = OperationStage::DownloadLibs;

impl<'a, 'b> LibsData<'a, 'b> {
    pub async fn sync(
        id: InstanceId,
        manifest: &'b serde_json::Value,
        paths: &'a LauncherPaths,
        provider: MetaProvider,
        db: Arc<Database>,
        op: &OperationHandle,
    ) -> Result<LibsOutcome, LibsSyncError> {
        let data = LibsData {
            manifest,
            paths,
            provider,
            current_os: construct_os_name()?,
        };

        let wanted = match provider {
            MetaProvider::Mojang => data.parse_manifest_official().await?,
            MetaProvider::Prism => data.parse_manifest_prism().await?,
        };

        Self::sync_prepared(id, wanted, db, op).await
    }

    pub async fn sync_prepared(
        id: InstanceId,
        wanted: Vec<LibInfo>,
        db: Arc<Database>,
        op: &OperationHandle,
    ) -> Result<LibsOutcome, LibsSyncError> {
        op.start_stage_determinable(STAGE_TYPE);
        let started = Instant::now();

        let mut unique = HashMap::new();

        for lib in &wanted {
            if let Some(previous) = unique.insert(lib.hash().clone(), lib) {
                if previous.path() != lib.path() || previous.is_native() != lib.is_native() {
                    return Err(LibsSyncError::ManifestMalformed(
                           "legacy DB cannot represent one hash at different paths/roles; use InstalledProfile".into(),
                       ));
                }
            }
        }

        let store = DbLibStore::new(db);

        let outcome = Syncer::default()
            .run(
                wanted.clone(),
                |lib| PathBuf::from(lib.path()),
                &store,
                op,
                STAGE_TYPE,
            )
            .await?;
        if !outcome.cached.is_empty() {
            crate::instance::download::sync::SyncStore::register(&store, &outcome.cached).await?;
        }

        let mut seen = std::collections::HashSet::new();

        let ordered: Vec<&LibInfo> = wanted
            .iter()
            .filter(|lib| seen.insert(lib.hash().clone()))
            .collect();

        let classpath = ordered
            .iter()
            .map(|lib| PathBuf::from(lib.path()))
            .collect();

        let natives = ordered
            .iter()
            .filter(|lib| lib.is_native())
            .map(|lib| PathBuf::from(lib.path()))
            .collect();

        store.link_instance(id, &ordered).await?;
        op.complete_stage(StageResult {
            status: Outcome::Completed,
            stage: STAGE_TYPE,
            duration_secs: started.elapsed().as_secs_f64(),
            error: None,
        });

        Ok(LibsOutcome { classpath, natives })
    }

    pub fn build_maven_file_path(&self, maven_path: &str) -> String {
        maven::build_file_path(self.paths.libraries(), maven_path)
    }
}

fn construct_os_name() -> Result<&'static str, LibsSyncError> {
    // Linux
    #[cfg(all(target_os = "linux", target_arch = "x86"))]
    return Ok("linux");

    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    return Ok("linux");

    #[cfg(all(target_os = "linux", target_arch = "arm"))]
    return Ok("linux-arm32");

    #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
    return Ok("linux-arm64");

    // macOS
    #[cfg(all(target_os = "macos", target_arch = "x86"))]
    return Ok("osx");

    #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
    return Ok("osx");

    #[cfg(all(target_os = "macos", target_arch = "arm"))]
    return Ok("osx");

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    return Ok("osx-arm64");

    // Windows
    #[cfg(all(target_os = "windows", target_arch = "x86"))]
    return Ok("windows");

    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    return Ok("windows");

    #[cfg(all(target_os = "windows", target_arch = "arm"))]
    return Ok("windows-arm32");

    #[cfg(all(target_os = "windows", target_arch = "aarch64"))]
    return Ok("windows-arm64");

    // If OS/arch combination is not supported
    #[allow(unreachable_code)]
    Err(LibsSyncError::OsNotAvailable)
}

#[cfg(test)]
mod prepared_tests {
    use super::*;
    #[async_std::test]
    async fn prepared_files_repair_missing_db_rows_without_network() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("library.jar");
        std::fs::write(&file, b"fixture").unwrap();
        let hash = files::sha1_file(&file).unwrap();
        let db = Arc::new(Database::init("sqlite::memory:").await.unwrap());
        sqlx::query("INSERT INTO instances(id,name,dir,version,loader,manifest_url,meta_provider) VALUES(1,'prepared','prepared','1.20.1','vanilla','https://example.invalid/base.json','prism')").execute(&db.pool).await.unwrap();
        let operations = crate::data::registry::operation::OperationRegistry::new(
            crate::bus::EventBus::new(128),
        );
        let op = operations.begin("test".into(), vec![STAGE_TYPE]);
        let library = LibInfo {
            hash,
            name: "fixture".into(),
            path: file.to_string_lossy().into_owned(),
            url: "https://example.invalid/never-requested".into(),
            native: false,
        };
        let result = LibsData::sync_prepared(1.into(), vec![library], Arc::clone(&db), &op)
            .await
            .unwrap();
        assert_eq!(result.classpath, vec![file]);
        assert_eq!(linked_libs(1.into(), db).await.unwrap().len(), 1);
        op.finish(Outcome::Completed);
    }
    #[async_std::test]
    async fn legacy_cache_rejects_corrupted_file_despite_db_entry() {
        use crate::instance::download::sync::SyncStore;
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("library.jar");
        std::fs::write(&file, b"fixture").unwrap();
        let hash = files::sha1_file(&file).unwrap();
        let db = Arc::new(Database::init("sqlite::memory:").await.unwrap());
        let store = DbLibStore::new(db);
        let lib = LibInfo {
            hash,
            name: "fixture".into(),
            path: file.to_string_lossy().into_owned(),
            url: "https://example.invalid/unused".into(),
            native: false,
        };
        store.register(&[lib.clone()]).await.unwrap();
        std::fs::write(file, b"corrupted").unwrap();
        assert!(store.cached(&[lib]).await.unwrap().is_empty());
    }
}
