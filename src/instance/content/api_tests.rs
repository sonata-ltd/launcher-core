//! Интеграционные проверки локального протокола: реальные SQLite и файлы,
//! но без HTTP/запуска JVM. Каждый тест владеет отдельным временным корнем.
use super::{api::PublishedFile, recovery, service::InstanceContentService, storage};
use crate::{
    bus::EventBus,
    data::{
        config::Config, db::Database, layout::LauncherPaths, registry::operation::OperationRegistry,
    },
    instance::content::store::{storage as objects, ObjectStore},
    instance::{model::InstanceId, service::InstanceService, settings::service::SettingsService},
    java::service::JavaService,
    modloader::model::{LoaderKind, LoaderSpec},
    utils::cancellation::Cancellation,
};
use std::sync::Arc;

struct Fixture {
    _directory: tempfile::TempDir,
    paths: Arc<LauncherPaths>,
    db: Arc<Database>,
    instances: Arc<InstanceService>,
    operations: Arc<OperationRegistry>,
    id: InstanceId,
    root: std::path::PathBuf,
}

async fn fixture() -> Fixture {
    let directory = tempfile::tempdir().unwrap();
    let paths = Arc::new(
        LauncherPaths::resolve(&Config {
            root_override: Some(directory.path().into()),
            ..Config::init()
        })
        .await
        .unwrap(),
    );
    let db = Arc::new(Database::init("sqlite::memory:").await.unwrap());
    let bus = EventBus::new(64);
    let operations = Arc::new(OperationRegistry::new(bus.clone()));
    let java = Arc::new(JavaService::new(
        db.clone(),
        bus.clone(),
        paths.java_runtimes().clone(),
    ));
    let instances = Arc::new(InstanceService::new(
        db.clone(),
        Arc::new(SettingsService::new(db.clone())),
        java,
        operations.clone(),
        paths.clone(),
        bus,
    ));
    let record = instances
        .create_with_loader(
            "Fixture".into(),
            "1.21.1".into(),
            LoaderSpec {
                kind: LoaderKind::Fabric,
                version: Some("0.16.9".into()),
            },
            "https://example.invalid/unused".into(),
            "prism".into(),
        )
        .await
        .unwrap();
    let root = paths.instances().join(&record.dir);
    std::fs::create_dir_all(root.join("mods")).unwrap();
    Fixture {
        _directory: directory,
        paths,
        db,
        instances,
        operations,
        id: record.id,
        root,
    }
}

async fn prepared(fixture: &Fixture) -> (uuid::Uuid, Vec<PublishedFile>) {
    let source = fixture.root.join("source.jar");
    std::fs::write(&source, b"verified bytes").unwrap();
    let store = ObjectStore::new(fixture.paths.root().join("content-store"));
    let object = store
        .import_file(&source, None, &Cancellation::default())
        .unwrap();
    let object_id = objects::register(&fixture.db, &object).await.unwrap();
    let files = vec![PublishedFile {
        name: "mod.jar".into(),
        sha256: object.digests.sha256,
        object_id,
        provider: "modrinth".into(),
        project_id: "project".into(),
        release_id: "version".into(),
        placement: "hardlink".into(),
    }];
    let id = uuid::Uuid::new_v4();
    storage::prepare(&fixture.db, id, fixture.id, &files)
        .await
        .unwrap();
    (id, files)
}

#[async_std::test]
async fn mutation_lease_prevents_launch_and_loader_install() {
    let fixture = fixture().await;
    let lease = fixture
        .instances
        .reserve_content_change(fixture.id)
        .unwrap();
    assert!(fixture
        .instances
        .launch(fixture.id)
        .await
        .unwrap_err()
        .to_string()
        .contains("content installation"));
    assert!(fixture
        .instances
        .install(fixture.id)
        .await
        .unwrap_err()
        .to_string()
        .contains("content installation"));
    assert!(fixture
        .instances
        .reserve_content_change(fixture.id)
        .is_err());
    drop(lease);
    assert!(fixture.instances.reserve_content_change(fixture.id).is_ok());
}

#[async_std::test]
async fn unfinished_publication_blocks_launch_and_install_until_recovery() {
    let fixture = fixture().await;
    let (operation, files) = prepared(&fixture).await;
    assert!(storage::ensure_no_pending(&fixture.db, fixture.id)
        .await
        .is_err());
    use crate::instance::error::InstanceError;
    assert!(matches!(
        fixture.instances.launch(fixture.id).await,
        Err(InstanceError::ContentRecoveryPending(id)) if id == fixture.id
    ));
    assert!(matches!(
        fixture.instances.install(fixture.id).await,
        Err(InstanceError::ContentRecoveryPending(id)) if id == fixture.id
    ));
    recovery::recover(&fixture.db, &fixture.paths, &fixture.instances, None)
        .await
        .unwrap();
    assert_eq!(
        std::fs::read(fixture.root.join("mods/mod.jar")).unwrap(),
        b"verified bytes"
    );
    assert!(storage::operation_completed(&fixture.db, operation)
        .await
        .unwrap());
    storage::ensure_no_pending(&fixture.db, fixture.id)
        .await
        .unwrap();
    // Повторный запуск recovery ничего не дублирует.
    recovery::recover(&fixture.db, &fixture.paths, &fixture.instances, None)
        .await
        .unwrap();
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM instance_content")
        .fetch_one(&fixture.db.pool)
        .await
        .unwrap();
    assert_eq!(count, 1);
    let placement: String = sqlx::query_scalar("SELECT placement FROM instance_content")
        .fetch_one(&fixture.db.pool)
        .await
        .unwrap();
    assert_eq!(placement, "copy");

    let store = super::store::ObjectStore::new(fixture.paths.root().join("content-store"));
    let object_path = store.path_for(&files[0].sha256).unwrap();
    std::fs::write(fixture.root.join("mods/mod.jar"), b"modified locally").unwrap();
    assert_eq!(std::fs::read(object_path).unwrap(), b"verified bytes");

    storage::clear_completed(&fixture.db, Some(operation))
        .await
        .unwrap();
    // История успешно законченной операции не должна навсегда запрещать
    // удаление экземпляра внешним lifecycle-сервисом.
    sqlx::query("DELETE FROM instances WHERE id=?")
        .bind(fixture.id.get())
        .execute(&fixture.db.pool)
        .await
        .unwrap();
}

#[async_std::test]
async fn recovery_never_overwrites_a_changed_user_file() {
    let fixture = fixture().await;
    prepared(&fixture).await;
    let target = fixture.root.join("mods/mod.jar");
    std::fs::write(&target, b"user changes").unwrap();
    let error = recovery::recover(&fixture.db, &fixture.paths, &fixture.instances, None)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("RecoveryConflict"));
    assert_eq!(std::fs::read(target).unwrap(), b"user changes");
    assert!(storage::ensure_no_pending(&fixture.db, fixture.id)
        .await
        .is_err());
}

#[async_std::test]
async fn second_service_cannot_use_the_same_launcher_root() {
    let fixture = fixture().await;
    let first = InstanceContentService::new(
        fixture.db.clone(),
        fixture.paths.clone(),
        fixture.instances.clone(),
        None,
        fixture.operations.clone(),
    )
    .await
    .unwrap();
    assert!(InstanceContentService::new(
        fixture.db.clone(),
        fixture.paths.clone(),
        fixture.instances.clone(),
        None,
        fixture.operations.clone()
    )
    .await
    .is_err());
    drop(first);
    assert!(InstanceContentService::new(
        fixture.db.clone(),
        fixture.paths.clone(),
        fixture.instances.clone(),
        None,
        fixture.operations.clone()
    )
    .await
    .is_ok());
}

#[test]
fn transfer_and_filesystem_steps_observe_the_same_cancellation_flag() {
    let token = crate::utils::download::transfer::Cancel::default();
    let file_token = Cancellation::from_flag(token.0.clone());
    token.cancel();
    assert!(file_token.check().is_err());
}
