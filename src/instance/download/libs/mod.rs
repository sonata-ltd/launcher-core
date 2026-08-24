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
    utils::{download::Downloadable, maven},
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
        op.start_stage(STAGE_TYPE);
        let started = Instant::now();

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

        let order: Vec<String> = wanted.iter().map(|lib| lib.hash().clone()).collect();

        let store = DbLibStore::new(db);
        let outcome = Syncer::default()
            .run(
                wanted,
                |lib| PathBuf::from(lib.path()),
                &store,
                op,
                STAGE_TYPE,
            )
            .await?;

        let mut by_hash: HashMap<&str, &LibInfo> =
            HashMap::with_capacity(outcome.cached.len() + outcome.downloaded.len());

        for lib in outcome.all() {
            by_hash.entry(lib.hash().as_str()).or_insert(lib);
        }

        let mut ordered: Vec<&LibInfo> = Vec::with_capacity(order.len());
        let mut classpath = Vec::with_capacity(order.len());
        let mut natives = Vec::new();

        for hash in &order {
            let Some(lib) = by_hash.remove(hash.as_str()) else {
                continue;
            };

            let path = PathBuf::from(lib.path());

            if lib.is_native() {
                natives.push(path.clone());
            }

            classpath.push(path);
            ordered.push(lib);
        }

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
