use std::sync::Arc;

use super::*;
use crate::{
    bus::EventBus,
    data::{
        config::Config,
        db::Database,
        layout::LauncherPaths,
        registry::operation::{
            message::{stage::OperationStage, status::Outcome},
            OperationRegistry,
        },
    },
    instance::{
        download::transfer::Cancel,
        init::{
            error::InitError,
            install::{InstallContext, InstallRequest},
            model::{InstalledArtifact, InstalledProfile},
            workspace::InstallWorkspace,
        },
    },
    modloader::{
        catalog::ModloaderCatalog,
        error::ModloaderError,
        model::{JavaPolicy, LoaderKind, LoaderSpec},
    },
    utils::files,
    version::provider::MetaProvider,
};
use serde_json::json;

#[async_std::test]
async fn failed_reinstall_removes_new_generation_and_preserves_published_installation() {
    let temp = tempfile::tempdir().unwrap();
    let paths = LauncherPaths::resolve(&Config {
        root_override: Some(temp.path().to_path_buf()),
        ..Config::init()
    })
    .await
    .unwrap();
    let db = Arc::new(Database::init("sqlite::memory:").await.unwrap());
    sqlx::query(
        "INSERT INTO instances(id,name,dir,version,loader,loader_version,manifest_url,meta_provider)
         VALUES(1,'test','test','1.12.2','cleanroom','fixture','https://example.invalid/unused','prism')",
    )
    .execute(&db.pool)
    .await
    .unwrap();

    let previous_workspace = InstallWorkspace::create(&paths, 1.into()).await.unwrap();
    let previous_runtime = previous_workspace.maven_dir().join("client.jar");

    std::fs::write(&previous_runtime, b"previous runtime").unwrap();
    previous_workspace.cleanup_work().await.unwrap();

    let previous_profile = InstalledProfile {
        format_version: 1,
        minecraft: "1.12.2".into(),
        loader: LoaderSpec {
            kind: LoaderKind::Cleanroom,
            version: Some("fixture".into()),
        },
        manifest: json!({"mainClass": "previous.Main"}),
        java: JavaPolicy {
            majors: vec![21],
            allow_newer: false,
        },
        libraries_root: previous_workspace.relative_maven_dir().to_path_buf(),
        artifacts: vec![InstalledArtifact {
            path: "client.jar".into(),
            sha1: files::sha1_file(&previous_runtime).unwrap(),
            classpath: true,
            native: false,
        }],
        metadata: vec![],
    };

    storage::publish(&db, 1.into(), &previous_profile)
        .await
        .unwrap();

    let stored_before: String =
        sqlx::query_scalar("SELECT profile_json FROM instance_installations WHERE instance_id = 1")
            .fetch_one(&db.pool)
            .await
            .unwrap();

    // No registered Cleanroom release: fail after creating the new workspace,
    // before any downloads or Java processes can run.
    let catalog = ModloaderCatalog::new(paths.meta().join("modloader-catalog"));
    let operations = OperationRegistry::new(EventBus::new(32));
    let op = operations.begin(
        "instance.install".into(),
        vec![OperationStage::FetchManifest],
    );
    let request = InstallRequest {
        instance_id: 1.into(),
        instance_dir_name: "test".into(),
        game_version: "1.12.2".into(),
        manifest_url: "https://example.invalid/unused".into(),
        meta_provider: MetaProvider::Prism,
        loader: previous_profile.loader.clone(),
    };
    let cancel = Cancel::default();
    let context = InstallContext {
        paths: &paths,
        db: Arc::clone(&db),
        catalog: &catalog,
        modloader_java_runtimes: &[],
        op: &op,
        cancel: &cancel,
    };

    let error = install::execute(&request, &context).await.unwrap_err();

    assert!(matches!(
        error,
        InitError::Loader(ModloaderError::Invalid(message))
            if message.contains("Cleanroom release is not registered")
    ));
    let stored_after: String =
        sqlx::query_scalar("SELECT profile_json FROM instance_installations WHERE instance_id = 1")
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(stored_after, stored_before);
    let profile = storage::load(&db, 1.into()).await.unwrap().unwrap();
    assert_eq!(profile.libraries_root, previous_profile.libraries_root);
    assert_eq!(
        std::fs::read(previous_runtime).unwrap(),
        b"previous runtime"
    );
    let generations: Vec<_> = std::fs::read_dir(paths.libraries().join("instances/1"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    assert_eq!(
        generations,
        vec![previous_workspace
            .maven_dir()
            .parent()
            .unwrap()
            .to_path_buf()]
    );
    op.finish(Outcome::Failed);
}

async fn empty_installation() -> (
    tempfile::TempDir,
    LauncherPaths,
    Arc<Database>,
    ModloaderCatalog,
) {
    let temp = tempfile::tempdir().unwrap();
    let paths = LauncherPaths::resolve(&Config {
        root_override: Some(temp.path().to_path_buf()),
        ..Config::init()
    })
    .await
    .unwrap();
    let db = Arc::new(Database::init("sqlite::memory:").await.unwrap());
    let catalog = ModloaderCatalog::new(paths.meta().join("modloader-catalog"));
    (temp, paths, db, catalog)
}

fn vanilla_request() -> InstallRequest {
    InstallRequest {
        instance_id: 1.into(),
        instance_dir_name: "test".into(),
        game_version: "1.20.1".into(),
        manifest_url: "https://example.invalid/unused".into(),
        meta_provider: MetaProvider::Prism,
        loader: LoaderSpec::vanilla(),
    }
}

#[async_std::test]
async fn cancelled_installation_returns_typed_error_without_creating_workspace() {
    let (_temp, paths, db, catalog) = empty_installation().await;
    let operations = OperationRegistry::new(EventBus::new(32));
    let op = operations.begin("instance.install".into(), vec![]);
    let cancel = Cancel::default();
    cancel.cancel();
    let context = InstallContext {
        paths: &paths,
        db,
        catalog: &catalog,
        modloader_java_runtimes: &[],
        op: &op,
        cancel: &cancel,
    };

    let error = install::execute(&vanilla_request(), &context)
        .await
        .unwrap_err();

    assert!(error.is_cancelled());
    assert!(matches!(
        error,
        InitError::Download(crate::instance::download::error::DownloadError::Cancelled)
    ));
    assert!(!paths.libraries().exists());
    op.finish(Outcome::Cancelled);
}

#[async_std::test]
async fn database_read_failure_keeps_source_error_and_cleans_new_generation() {
    let (_temp, paths, db, catalog) = empty_installation().await;
    // Fail the first storage read, after workspace creation and before metadata
    // requests. No network, assets downloads or Java processes are needed.
    db.pool.close().await;
    let operations = OperationRegistry::new(EventBus::new(32));
    let op = operations.begin("instance.install".into(), vec![]);
    let cancel = Cancel::default();
    let context = InstallContext {
        paths: &paths,
        db,
        catalog: &catalog,
        modloader_java_runtimes: &[],
        op: &op,
        cancel: &cancel,
    };

    let error = install::execute(&vanilla_request(), &context)
        .await
        .unwrap_err();

    assert!(!error.is_cancelled());
    assert!(matches!(
        error,
        InitError::Db(crate::data::db::DbError::Db(sqlx::Error::PoolClosed))
    ));
    assert_eq!(
        std::fs::read_dir(paths.libraries().join("instances/1"))
            .unwrap()
            .count(),
        0
    );
    op.finish(Outcome::Failed);
}
