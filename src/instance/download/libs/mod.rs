use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
    time::Instant,
};

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
    Rules(#[from] crate::version::rules::RuleError),

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

    fn cache_key(&self) -> String {
        // Length prefix makes paths containing arbitrary delimiters unambiguous.
        format!("{}:{}:{}", self.hash.len(), self.hash, self.path)
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
            if let Some(previous) = unique.insert(lib.path().clone(), lib) {
                if previous.hash() != lib.hash() {
                    return Err(LibsSyncError::ManifestMalformed(
                        "different library bytes target the same path".into(),
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
            .filter(|lib| seen.insert((lib.cache_key(), lib.is_native())))
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

    #[deprecated(note = "use a checked relative path for file destinations")]
    #[allow(deprecated)]
    pub fn build_maven_file_path(&self, maven_path: &str) -> String {
        maven::build_file_path(self.paths.libraries(), maven_path)
    }
}

fn library_path(root: &Path, descriptor_path: &str) -> Result<String, LibsSyncError> {
    if descriptor_path.contains('\\') {
        return Err(LibsSyncError::ManifestMalformed(
            "invalid library path separator".into(),
        ));
    }
    let relative = Path::new(descriptor_path.trim_start_matches('/'));
    files::child(root, relative)
        .map_err(|error| LibsSyncError::ManifestMalformed(error.to_string()))?
        .into_os_string()
        .into_string()
        .map_err(|_| LibsSyncError::ManifestMalformed("non-Unicode library path".into()))
}

fn construct_os_name() -> Result<&'static str, LibsSyncError> {
    use crate::utils::platform::{Arch, HostPlatform, Os};
    let host = HostPlatform::current();
    match (host.os, host.arch) {
        (Os::Linux, Arch::X86 | Arch::X86_64) => Ok("linux"),
        (Os::Linux, Arch::Arm) => Ok("linux-arm32"),
        (Os::Linux, Arch::Aarch64) => Ok("linux-arm64"),
        (Os::MacOs, Arch::X86 | Arch::X86_64 | Arch::Arm) => Ok("osx"),
        (Os::MacOs, Arch::Aarch64) => Ok("osx-arm64"),
        (Os::Windows, Arch::X86 | Arch::X86_64) => Ok("windows"),
        (Os::Windows, Arch::Arm) => Ok("windows-arm32"),
        (Os::Windows, Arch::Aarch64) => Ok("windows-arm64"),
        _ => Err(LibsSyncError::OsNotAvailable),
    }
}

#[cfg(test)]
mod prepared_tests {
    use super::*;

    #[async_std::test]
    async fn identical_bytes_keep_distinct_instance_paths_roles_and_positions() {
        let temp = tempfile::tempdir().unwrap();
        let db = Arc::new(Database::init("sqlite::memory:").await.unwrap());
        for id in [1, 2] {
            sqlx::query("INSERT INTO instances(id,name,dir,version,loader,manifest_url,meta_provider) VALUES(?,?,?,'1.21','vanilla','https://example.invalid','prism')")
                .bind(id).bind(id.to_string()).bind(id.to_string()).execute(&db.pool).await.unwrap();
        }
        let operations = crate::data::registry::operation::OperationRegistry::new(
            crate::bus::EventBus::new(128),
        );
        let first = temp.path().join("first.jar");
        let second = temp.path().join("second.jar");
        for path in [&first, &second] {
            std::fs::write(path, b"same bytes").unwrap();
        }
        let hash = files::sha1_file(&first).unwrap();
        let library = |path: &Path, native| {
            LibInfo::new(
                "fixture".into(),
                hash.clone(),
                path.into(),
                "not-a-network-url".into(),
                native,
            )
            .unwrap()
        };
        let first_libs = vec![
            library(&first, false),
            library(&second, true),
            library(&first, true),
        ];
        let op = operations.begin("test".into(), vec![STAGE_TYPE]);
        LibsData::sync_prepared(1.into(), first_libs.clone(), db.clone(), &op)
            .await
            .unwrap();
        op.finish(Outcome::Completed);
        let op = operations.begin("test".into(), vec![STAGE_TYPE]);
        LibsData::sync_prepared(2.into(), vec![library(&second, false)], db.clone(), &op)
            .await
            .unwrap();
        op.finish(Outcome::Completed);
        assert_eq!(linked_libs(1.into(), db.clone()).await.unwrap(), first_libs);
        assert_eq!(
            linked_libs(2.into(), db.clone()).await.unwrap(),
            vec![library(&second, false)]
        );
        // Removing B's snapshot must not delete A's roles or placements.
        sqlx::query("DELETE FROM instances WHERE id=2")
            .execute(&db.pool)
            .await
            .unwrap();
        assert_eq!(linked_libs(1.into(), db).await.unwrap(), first_libs);
    }

    #[async_std::test]
    async fn missing_destination_is_not_a_cache_hit_for_identical_bytes_elsewhere() {
        use crate::instance::download::sync::SyncStore;
        let temp = tempfile::tempdir().unwrap();
        let existing = temp.path().join("existing.jar");
        std::fs::write(&existing, b"bytes").unwrap();
        let hash = files::sha1_file(&existing).unwrap();
        let present = LibInfo::new(
            "present".into(),
            hash.clone(),
            existing,
            "unused".into(),
            false,
        )
        .unwrap();
        let missing = LibInfo::new(
            "missing".into(),
            hash,
            temp.path().join("missing.jar"),
            "unused".into(),
            false,
        )
        .unwrap();
        let db = Arc::new(Database::init("sqlite::memory:").await.unwrap());
        let found = DbLibStore::new(db)
            .cached(&[present.clone(), missing.clone()])
            .await
            .unwrap();
        assert!(found.contains(&present.cache_key()));
        assert!(!found.contains(&missing.cache_key()));
    }

    #[test]
    fn legacy_descriptor_paths_are_checked_without_reinterpreting_coordinates() {
        let root = tempfile::tempdir().unwrap();
        assert_eq!(
            PathBuf::from(library_path(root.path(), "/org/example/library.jar").unwrap()),
            root.path().join("org/example/library.jar")
        );
        assert!(library_path(root.path(), "/../library.jar").is_err());
        assert!(library_path(root.path(), "C:\\outside.jar").is_err());
    }

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
