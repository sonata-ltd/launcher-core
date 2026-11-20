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

async fn install_asset_fixture(
    paths: &LauncherPaths,
    db: Arc<Database>,
    server: &httpmock::MockServer,
    route: &str,
    id: &str,
    body: &str,
    expectation: (usize, String),
) -> Result<InstalledProfile, InitError> {
    let (size, sha1) = expectation;
    let index_path = format!("/{route}/index.json");
    let version_path = format!("/{route}/version.json");
    let _index = server.mock(|when, then| {
        when.path(&index_path);
        then.status(200).body(body);
    });
    let _version = server.mock(|when, then| {
        when.path(&version_path);
        then.status(200).json_body(json!({
            "mainClass": "Fixture",
            "javaVersion": {"majorVersion": 17},
            "arguments": {"game": ["--assetIndex", "${assets_index_name}"]},
            "assetIndex": {"id": id, "url": server.url(index_path), "size": size, "sha1": sha1},
        }));
    });
    let operations = OperationRegistry::new(EventBus::new(32));
    let op = operations.begin("instance.install".into(), vec![]);
    let cancel = Cancel::new();
    let java_use = crate::java::managed::process::JavaRetention::new(
        db.clone(),
        paths.java_runtimes().clone(),
    )
    .begin(&cancel)
    .await
    .unwrap();
    let request = InstallRequest {
        meta_provider: MetaProvider::Mojang,
        manifest_url: server.url(version_path),
        ..vanilla_request()
    };
    let catalog = ModloaderCatalog::new(paths.meta().join("modloader-catalog"));
    let result = install::execute(
        &request,
        &InstallContext {
            paths,
            db,
            catalog: &catalog,
            modloader_java_runtimes: &[],
            java_use: &java_use,
            op: &op,
            cancel: &cancel,
        },
    )
    .await;
    java_use.finish(true);
    op.finish(if result.is_ok() {
        Outcome::Completed
    } else {
        Outcome::Failed
    });
    result
}

fn fixture_sha1(body: &str) -> String {
    use sha1::{Digest, Sha1};
    format!("{:x}", Sha1::digest(body.as_bytes()))
}

#[async_std::test]
async fn asset_index_install_publication_matches_prepared_game_id_after_restart() {
    use crate::{instance::model::InstanceRecord, java::model::JavaRuntime};
    const BODY: &str = "{ \"objects\": {}, \"virtual\": false }\n";
    let (temp, paths, _, _) = empty_installation().await;
    let url = format!("sqlite://{}", temp.path().join("test.sqlite").display());
    let db = Arc::new(Database::init(&url).await.unwrap());
    sqlx::query(
        "INSERT INTO instances(id,name,dir,version,loader,manifest_url,meta_provider)
         VALUES(1,'test','test','1.20.1','vanilla','fixture','prism')",
    )
    .execute(&db.pool)
    .await
    .unwrap();
    let server = httpmock::MockServer::start();
    install_asset_fixture(
        &paths,
        db.clone(),
        &server,
        "first",
        "actual-game-id",
        BODY,
        (BODY.len(), fixture_sha1(BODY)),
    )
    .await
    .unwrap();
    drop(db);
    let db = Arc::new(Database::init(&url).await.unwrap());
    let record = InstanceRecord {
        id: 1.into(),
        name: "test".into(),
        dir: "test".into(),
        version: "1.20.1".into(),
        loader: LoaderKind::Vanilla,
        loader_version: None,
        manifest_url: String::new(),
        meta_provider: "prism".into(),
    };
    let profile = storage::load(&db, record.id).await.unwrap().unwrap();
    let command = crate::instance::launch::resolved::prepare(
        &record,
        &crate::instance::settings::EffectiveSettings {
            java_runtime: Some(1),
            memory_min: 512,
            memory_max: 1024,
            jvm_args: vec![],
        },
        &JavaRuntime {
            id: 1,
            major: 17,
            version: "17".into(),
            arch: None,
            exec_path: "java".into(),
            home_path: String::new(),
            vendor: None,
            source: Default::default(),
        },
        &paths,
        profile.clone(),
    )
    .await
    .unwrap();
    let game_id = &command
        .args
        .windows(2)
        .find(|pair| pair[0] == "--assetIndex")
        .unwrap()[1];
    assert_eq!(game_id, "actual-game-id");
    let published = paths
        .assets()
        .join("indexes")
        .join(format!("{game_id}.json"));
    assert_eq!(std::fs::read(&published).unwrap(), BODY.as_bytes());

    const OTHER: &str = "{\"objects\":{}}";
    for (route, id, size, sha1, succeeds) in [
        (
            "bad-size",
            "actual-game-id",
            OTHER.len() + 1,
            fixture_sha1(OTHER),
            false,
        ),
        (
            "bad-hash",
            "actual-game-id",
            OTHER.len(),
            fixture_sha1("different"),
            false,
        ),
        (
            "conflict",
            "actual-game-id",
            OTHER.len(),
            fixture_sha1(OTHER),
            false,
        ),
        (
            "second",
            "second-game-id",
            OTHER.len(),
            fixture_sha1(OTHER),
            true,
        ),
    ] {
        let result =
            install_asset_fixture(&paths, db.clone(), &server, route, id, OTHER, (size, sha1))
                .await;
        assert_eq!(result.is_ok(), succeeds, "{result:?}");
        assert_eq!(std::fs::read(&published).unwrap(), BODY.as_bytes());
        let current = storage::load(&db, record.id).await.unwrap().unwrap();
        if !succeeds {
            assert_eq!(current.libraries_root, profile.libraries_root);
        } else {
            assert_eq!(
                std::fs::read(paths.assets().join("indexes/second-game-id.json")).unwrap(),
                OTHER.as_bytes()
            );
            assert_ne!(current.libraries_root, profile.libraries_root);
        }
    }
}

#[async_std::test]
async fn asset_index_install_rejects_duplicate_keys_before_persistence() {
    use crate::instance::download::manifest::ManifestError;

    let (_temp, paths, db, _) = empty_installation().await;
    sqlx::query(
        "INSERT INTO instances(id,name,dir,version,loader,manifest_url,meta_provider)
         VALUES(1,'test','test','1.20.1','vanilla','fixture','prism')",
    )
    .execute(&db.pool)
    .await
    .unwrap();
    let server = httpmock::MockServer::start();
    for body in [
        r#"{"objects":{"foo":{"hash":"0000000000000000000000000000000000000000"}},"objects":{}}"#,
        r#"{"objects":{"foo":{"hash":"0000000000000000000000000000000000000000"},"foo":{"hash":"0000000000000000000000000000000000000000"}}}"#,
        r#"{"objects":{"foo":{"hash":"0000000000000000000000000000000000000000","hash":"0000000000000000000000000000000000000000"}}}"#,
    ] {
        let result = install_asset_fixture(
            &paths,
            db.clone(),
            &server,
            &format!("bad-{}", body.len()),
            "invalid",
            body,
            (body.len(), fixture_sha1(body)),
        )
        .await;
        assert!(
            matches!(
                result,
                Err(InitError::Manifest(ManifestError::InvalidAssetIndexJson(_)))
            ),
            "{result:?}"
        );
    }
    assert!(storage::load(&db, 1.into()).await.unwrap().is_none());
    assert!(!paths.assets().join("indexes").exists());
}

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

    retention::attach(db.clone(), paths.libraries().clone(), Cancel::new())
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
    let java_use = crate::java::managed::process::JavaRetention::new(
        db.clone(),
        paths.java_runtimes().clone(),
    )
    .begin(&cancel)
    .await
    .unwrap();
    let context = InstallContext {
        paths: &paths,
        db: Arc::clone(&db),
        catalog: &catalog,
        modloader_java_runtimes: &[],
        java_use: &java_use,
        op: &op,
        cancel: &cancel,
    };

    let error = install::execute(&request, &context).await.unwrap_err();
    java_use.finish(true);

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
    let java_use = crate::java::managed::process::JavaRetention::new(
        db.clone(),
        paths.java_runtimes().clone(),
    )
    .begin(&cancel)
    .await
    .unwrap();
    cancel.cancel();
    let context = InstallContext {
        paths: &paths,
        db,
        catalog: &catalog,
        modloader_java_runtimes: &[],
        java_use: &java_use,
        op: &op,
        cancel: &cancel,
    };

    let error = install::execute(&vanilla_request(), &context)
        .await
        .unwrap_err();

    assert!(error.is_cancelled());
    java_use.finish(true);
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
    let java_use = crate::java::managed::process::JavaRetention::new(
        db.clone(),
        paths.java_runtimes().clone(),
    )
    .begin(&Cancel::new())
    .await
    .unwrap();
    // Fail the owner/storage read before outputs or metadata requests. No
    // network, assets downloads or Java processes are needed.
    db.pool.close().await;
    let operations = OperationRegistry::new(EventBus::new(32));
    let op = operations.begin("instance.install".into(), vec![]);
    let cancel = Cancel::default();
    let context = InstallContext {
        paths: &paths,
        db,
        catalog: &catalog,
        modloader_java_runtimes: &[],
        java_use: &java_use,
        op: &op,
        cancel: &cancel,
    };

    let error = install::execute(&vanilla_request(), &context)
        .await
        .unwrap_err();

    assert!(!error.is_cancelled());
    java_use.finish(true);
    assert!(matches!(
        error,
        InitError::Db(crate::data::db::DbError::Db(sqlx::Error::PoolClosed))
    ));
    assert!(!paths.libraries().join("instances/1").exists());
    op.finish(Outcome::Failed);
}
