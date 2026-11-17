//! Интеграционные проверки локального протокола: реальные SQLite и файлы,
//! но без HTTP/запуска JVM. Каждый тест владеет отдельным временным корнем.
use super::{journal::PublishedFile, recovery, service::InstanceContentService, storage};
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
        provider: super::model::Provider::Modrinth,
        project_id: "project".into(),
        release_id: "version".into(),
        placement: super::journal::Placement::Hardlink,
        dependencies: None,
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
        .activity()
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
        .activity()
        .reserve_content_change(fixture.id)
        .is_err());
    drop(lease);
    assert!(fixture
        .instances
        .activity()
        .reserve_content_change(fixture.id)
        .is_ok());
}

#[async_std::test]
async fn unfinished_publication_blocks_launch_and_install_until_recovery() {
    let fixture = fixture().await;
    let (operation, files) = prepared(&fixture).await;
    sqlx::query("UPDATE content_operations SET recovery_json=? WHERE id=?")
        .bind(serde_json::to_string(&files).unwrap())
        .bind(operation.to_string())
        .execute(&fixture.db.pool)
        .await
        .unwrap();
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
    assert!(matches!(
        fixture.instances.unregister(fixture.id).await,
        Err(InstanceError::ContentRecoveryPending(id)) if id == fixture.id
    ));
    recovery::recover(
        &fixture.db,
        &fixture.paths,
        &fixture.instances.get_registry(),
        None,
    )
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
    recovery::recover(
        &fixture.db,
        &fixture.paths,
        &fixture.instances.get_registry(),
        None,
    )
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
    fixture.instances.unregister(fixture.id).await.unwrap();
    assert!(fixture.root.join("mods/mod.jar").is_file());
}

#[async_std::test]
async fn recovery_never_overwrites_a_changed_user_file() {
    let fixture = fixture().await;
    prepared(&fixture).await;
    let target = fixture.root.join("mods/mod.jar");
    std::fs::write(&target, b"user changes").unwrap();
    let failures = recovery::recover(
        &fixture.db,
        &fixture.paths,
        &fixture.instances.get_registry(),
        None,
    )
    .await
    .unwrap();
    assert!(matches!(
        failures.as_slice(),
        [failure] if matches!(&failure.error, super::error::ContentError::RecoveryConflict { name } if name == "mod.jar")
    ));
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

#[async_std::test]
async fn unreadable_journal_does_not_prevent_other_instances_from_recovering() {
    let fixture = fixture().await;
    let (broken, files) = prepared(&fixture).await;
    let other = fixture
        .instances
        .create(
            "Other".into(),
            "1.21.1".into(),
            "https://example.invalid/unused".into(),
            "prism".into(),
        )
        .await
        .unwrap();
    let update = "UPDATE content_operations SET recovery_json=? WHERE id=?";
    sqlx::query(update)
        .bind(r#"{"version":42,"files":{"future":"format"}}"#)
        .bind(broken.to_string())
        .execute(&fixture.db.pool)
        .await
        .unwrap();
    let valid = uuid::Uuid::new_v4();
    storage::prepare(&fixture.db, valid, other.id, &files)
        .await
        .unwrap();

    let failures = recovery::recover(
        &fixture.db,
        &fixture.paths,
        &fixture.instances.get_registry(),
        None,
    )
    .await
    .unwrap();
    assert_eq!(failures.len(), 1);
    assert_eq!(failures[0].operation, broken.to_string());
    assert!(matches!(
        failures[0].error,
        super::error::ContentError::UnsupportedJournalVersion(42)
    ));
    assert!(storage::operation_completed(&fixture.db, valid)
        .await
        .unwrap());
    assert!(storage::has_pending(&fixture.db, fixture.id).await.unwrap());

    let valid_again = uuid::Uuid::new_v4();
    sqlx::query("DELETE FROM instance_content WHERE instance_id=?")
        .bind(other.id.get())
        .execute(&fixture.db.pool)
        .await
        .unwrap();
    storage::prepare(&fixture.db, valid_again, other.id, &files)
        .await
        .unwrap();
    sqlx::query(update)
        .bind("{")
        .bind(broken.to_string())
        .execute(&fixture.db.pool)
        .await
        .unwrap();
    let failures = recovery::recover(
        &fixture.db,
        &fixture.paths,
        &fixture.instances.get_registry(),
        Some(other.id),
    )
    .await
    .unwrap();
    assert!(failures.is_empty());
    assert!(storage::operation_completed(&fixture.db, valid_again)
        .await
        .unwrap());
    let placement: String =
        sqlx::query_scalar("SELECT placement FROM instance_content WHERE instance_id=?")
            .bind(other.id.get())
            .fetch_one(&fixture.db.pool)
            .await
            .unwrap();
    assert_eq!(placement, "external");
}

#[async_std::test]
async fn user_can_retry_recovery_after_resolving_a_conflict() {
    let fixture = fixture().await;
    let (id, _) = prepared(&fixture).await;
    let target = fixture.root.join("mods/mod.jar");
    std::fs::write(&target, b"user changes").unwrap();

    let service = Arc::new(
        InstanceContentService::new(
            fixture.db.clone(),
            fixture.paths.clone(),
            fixture.instances.clone(),
            None,
            fixture.operations.clone(),
        )
        .await
        .unwrap(),
    );
    assert_eq!(
        service.pending_recovery(fixture.id).await.unwrap(),
        [id.to_string()]
    );
    assert!(matches!(
        service.retry_recovery(fixture.id).await,
        Err(super::error::ContentError::RecoveryConflict { .. })
    ));
    assert_eq!(std::fs::read(&target).unwrap(), b"user changes");

    std::fs::rename(&target, fixture.root.join("preserved.jar")).unwrap();
    service.retry_recovery(fixture.id).await.unwrap();
    assert!(service
        .pending_recovery(fixture.id)
        .await
        .unwrap()
        .is_empty());
    assert_eq!(std::fs::read(target).unwrap(), b"verified bytes");
}

#[async_std::test]
async fn journal_with_mismatched_object_is_rejected_before_publication() {
    let fixture = fixture().await;
    let (id, mut files) = prepared(&fixture).await;
    files[0].object_id += 1;
    sqlx::query("UPDATE content_operations SET recovery_json=? WHERE id=?")
        .bind(super::journal::encode(&files).unwrap())
        .bind(id.to_string())
        .execute(&fixture.db.pool)
        .await
        .unwrap();

    let failures = recovery::recover(
        &fixture.db,
        &fixture.paths,
        &fixture.instances.get_registry(),
        None,
    )
    .await
    .unwrap();
    assert_eq!(failures.len(), 1);
    assert_eq!(failures[0].operation, id.to_string());
    assert!(!fixture.root.join("mods/mod.jar").exists());
    assert!(storage::has_pending(&fixture.db, fixture.id).await.unwrap());
}

#[async_std::test]
async fn startup_detaches_registered_legacy_links_without_changing_bytes() {
    let fixture = fixture().await;
    let (id, files) = prepared(&fixture).await;
    let store = ObjectStore::new(fixture.paths.root().join("content-store"));
    let object = store.path_for(&files[0].sha256).unwrap();
    let target = fixture.root.join("mods/mod.jar");
    std::fs::hard_link(&object, &target).unwrap();
    storage::complete(&fixture.db, id, fixture.id, &files)
        .await
        .unwrap();

    let _service = InstanceContentService::new(
        fixture.db.clone(),
        fixture.paths.clone(),
        fixture.instances.clone(),
        None,
        fixture.operations.clone(),
    )
    .await
    .unwrap();
    assert_eq!(std::fs::read(&target).unwrap(), b"verified bytes");
    std::fs::write(&target, b"local changes").unwrap();
    assert_eq!(std::fs::read(&object).unwrap(), b"verified bytes");
    let placement: String = sqlx::query_scalar("SELECT placement FROM instance_content")
        .fetch_one(&fixture.db.pool)
        .await
        .unwrap();
    assert_eq!(placement, "copy");
}

#[async_std::test]
async fn abandoning_conflict_keeps_files_and_releases_only_the_selected_operation() {
    let fixture = fixture().await;
    let (id, _) = prepared(&fixture).await;
    let target = fixture.root.join("mods/mod.jar");
    std::fs::write(&target, b"local changes").unwrap();
    let service = InstanceContentService::new(
        fixture.db.clone(),
        fixture.paths.clone(),
        fixture.instances.clone(),
        None,
        fixture.operations.clone(),
    )
    .await
    .unwrap();

    assert!(service
        .abandon_recovery(999.into(), &id.to_string())
        .await
        .is_err());
    assert!(storage::has_pending(&fixture.db, fixture.id).await.unwrap());
    let busy = fixture
        .instances
        .activity()
        .reserve_content_change(fixture.id)
        .unwrap();
    assert!(service
        .abandon_recovery(fixture.id, &id.to_string())
        .await
        .is_err());
    drop(busy);
    service
        .abandon_recovery(fixture.id, &id.to_string())
        .await
        .unwrap();
    assert!(!storage::has_pending(&fixture.db, fixture.id).await.unwrap());
    assert_eq!(std::fs::read(target).unwrap(), b"local changes");
    let leases: i64 =
        sqlx::query_scalar("SELECT count(*) FROM content_object_leases WHERE operation_id=?")
            .bind(id.to_string())
            .fetch_one(&fixture.db.pool)
            .await
            .unwrap();
    assert_eq!(leases, 0);
}

#[async_std::test]
async fn database_enforces_publication_journal_state_transitions() {
    let fixture = fixture().await;
    let (id, files) = prepared(&fixture).await;
    let id_string = id.to_string();
    let update = "UPDATE content_operations SET state=? WHERE id=?";
    assert!(sqlx::query(update)
        .bind("unknown")
        .bind(&id_string)
        .execute(&fixture.db.pool)
        .await
        .is_err());
    storage::complete(&fixture.db, id, fixture.id, &files)
        .await
        .unwrap();
    assert!(sqlx::query(update)
        .bind("publishing")
        .bind(&id_string)
        .execute(&fixture.db.pool)
        .await
        .is_err());
    assert!(storage::operation_completed(&fixture.db, id).await.unwrap());
}

#[async_std::test]
async fn removed_registered_mod_does_not_block_publication_at_the_same_path() {
    let fixture = fixture().await;
    let (id, files) = prepared(&fixture).await;
    let target = fixture.root.join("mods/mod.jar");
    std::fs::write(&target, b"verified bytes").unwrap();
    storage::complete(&fixture.db, id, fixture.id, &files)
        .await
        .unwrap();

    let project = super::model::ProjectKey {
        provider: super::model::Provider::Modrinth,
        project_id: files[0].project_id.clone(),
    };
    let registered = storage::registered_mods(&fixture.db, fixture.id)
        .await
        .unwrap();
    let present = super::inventory::snapshot(fixture.root.clone())
        .await
        .unwrap();
    assert!(registered[0].matches(&project, &present));
    std::fs::remove_file(&target).unwrap();
    let missing = super::inventory::snapshot(fixture.root.clone())
        .await
        .unwrap();
    assert!(!registered[0].matches(&project, &missing));

    let replacement = uuid::Uuid::new_v4();
    storage::prepare(&fixture.db, replacement, fixture.id, &files)
        .await
        .unwrap();
    assert!(recovery::recover(
        &fixture.db,
        &fixture.paths,
        &fixture.instances.get_registry(),
        None
    )
    .await
    .unwrap()
    .is_empty());
    assert_eq!(std::fs::read(&target).unwrap(), b"verified bytes");
    assert!(storage::operation_completed(&fixture.db, replacement)
        .await
        .unwrap());
}

#[async_std::test]
async fn snapshot_detects_changed_bytes_even_with_the_same_size_and_modified_time() {
    let fixture = fixture().await;
    let target = fixture.root.join("mods/mod.jar");
    std::fs::write(&target, b"aaaa").unwrap();
    let modified = std::fs::metadata(&target).unwrap().modified().unwrap();
    let before = super::inventory::signature(fixture.root.clone())
        .await
        .unwrap();

    std::fs::write(&target, b"bbbb").unwrap();
    std::fs::OpenOptions::new()
        .write(true)
        .open(&target)
        .unwrap()
        .set_times(std::fs::FileTimes::new().set_modified(modified))
        .unwrap();
    assert_eq!(
        std::fs::metadata(&target).unwrap().modified().unwrap(),
        modified
    );
    let after = super::inventory::signature(fixture.root.clone())
        .await
        .unwrap();
    assert_ne!(before, after);
}

#[async_std::test]
async fn adoption_records_verified_local_mod_without_downloading_or_replacing_it() {
    use super::{
        catalog::types::{CatalogRelease, DownloadFile},
        install::PlannedRelease,
        model::{ProjectKey, Provider, ReleaseKey},
    };
    let fixture = fixture().await;
    let bytes = b"local mod";
    let target = fixture.root.join("mods/local.jar");
    std::fs::write(&target, bytes).unwrap();
    let signature = super::inventory::signature(fixture.root.clone())
        .await
        .unwrap();
    let (hashes, stamp) =
        crate::utils::files::digest::digest_file(&target, &Cancellation::default(), |_| {})
            .unwrap();
    let operation = uuid::Uuid::new_v4();
    let store = Arc::new(ObjectStore::new(fixture.paths.root().join("content-store")));
    super::install::execute(
        fixture.db.clone(),
        store.clone(),
        fixture.id,
        &fixture.root,
        operation,
        &[PlannedRelease {
            adopt_existing: true,
            release: CatalogRelease {
                key: ReleaseKey {
                    project: ProjectKey {
                        provider: Provider::Modrinth,
                        project_id: "project".into(),
                    },
                    release_id: "version".into(),
                },
                title: "Local mod".into(),
                file: DownloadFile {
                    file_name: "local.jar".into(),
                    url: "https://example.invalid/unreachable.jar".into(),
                    size_bytes: hashes.size_bytes,
                    sha1: Some(hashes.sha1),
                    sha512: Some(hashes.sha512),
                },
                dependencies: vec![],
            },
        }],
        &signature,
        &Default::default(),
        |_, _| {},
    )
    .await
    .unwrap();

    assert_eq!(std::fs::read(&target).unwrap(), bytes);
    let after =
        crate::utils::files::digest::FileStamp::from_metadata(&std::fs::metadata(&target).unwrap());
    assert_eq!(stamp, after);
    assert!(!store.path_for(&hashes.sha256).unwrap().exists());
    let row: (String, String, String) = sqlx::query_as(
        "SELECT i.placement, p.release_id, p.match_evidence FROM instance_content i
         JOIN content_provider_files p ON p.id=i.preferred_provider_file_id WHERE i.instance_id=?",
    )
    .bind(fixture.id.get())
    .fetch_one(&fixture.db.pool)
    .await
    .unwrap();
    assert_eq!(
        row,
        ("external".into(), "version".into(), "verified_hash".into())
    );
}

#[async_std::test]
async fn migration_detaches_a_modified_link_and_clears_stale_provenance() {
    let fixture = fixture().await;
    let (id, files) = prepared(&fixture).await;
    let store = ObjectStore::new(fixture.paths.root().join("content-store"));
    let object = store.path_for(&files[0].sha256).unwrap();
    let target = fixture.root.join("mods/mod.jar");
    std::fs::hard_link(&object, &target).unwrap();
    storage::complete(&fixture.db, id, fixture.id, &files)
        .await
        .unwrap();
    std::fs::write(&target, b"modified").unwrap();
    let _service = InstanceContentService::new(
        fixture.db.clone(),
        fixture.paths.clone(),
        fixture.instances.clone(),
        None,
        fixture.operations.clone(),
    )
    .await
    .unwrap();

    let row: (String, Option<i64>, Option<i64>) = sqlx::query_as(
        "SELECT placement, object_id, preferred_provider_file_id FROM instance_content",
    )
    .fetch_one(&fixture.db.pool)
    .await
    .unwrap();
    assert_eq!(row, ("external".into(), None, None));
    assert_eq!(std::fs::read(&target).unwrap(), b"modified");
    std::fs::write(&target, b"another change").unwrap();
    assert_eq!(std::fs::read(object).unwrap(), b"modified");
}

#[async_std::test]
async fn reconcile_tracks_disabled_renames_and_marks_a_removed_file_missing() {
    let fixture = fixture().await;
    let (id, files) = prepared(&fixture).await;
    let target = fixture.root.join("mods/mod.jar");
    std::fs::write(&target, b"verified bytes").unwrap();
    storage::complete(&fixture.db, id, fixture.id, &files)
        .await
        .unwrap();
    let disabled = fixture.root.join("mods/moved.jar.disabled");
    std::fs::rename(target, &disabled).unwrap();
    let snapshot = super::inventory::snapshot(fixture.root.clone())
        .await
        .unwrap();
    storage::reconcile_registered_mods(&fixture.db, fixture.id, &snapshot)
        .await
        .unwrap();

    let row: (String, bool, String) =
        sqlx::query_as("SELECT relative_path, mod_enabled, presence FROM instance_content")
            .fetch_one(&fixture.db.pool)
            .await
            .unwrap();
    assert_eq!(
        std::path::Path::new(&row.0),
        std::path::Path::new("mods").join("moved.jar.disabled")
    );
    assert!(!row.1);
    assert_eq!(row.2, "present");

    std::fs::remove_file(disabled).unwrap();
    let snapshot = super::inventory::snapshot(fixture.root.clone())
        .await
        .unwrap();
    storage::reconcile_registered_mods(&fixture.db, fixture.id, &snapshot)
        .await
        .unwrap();
    let presence: String = sqlx::query_scalar("SELECT presence FROM instance_content")
        .fetch_one(&fixture.db.pool)
        .await
        .unwrap();
    assert_eq!(presence, "missing");
}

#[cfg(any(target_os = "linux", target_os = "macos", windows))]
#[async_std::test]
async fn registered_mods_are_reconciled_before_launch_and_on_filesystem_notifications() {
    use std::time::Duration;

    let fixture = fixture().await;
    let (id, files) = prepared(&fixture).await;
    let target = fixture.root.join("mods/mod.jar");
    std::fs::write(&target, b"verified bytes").unwrap();
    storage::complete(&fixture.db, id, fixture.id, &files)
        .await
        .unwrap();

    let activity = fixture.instances.activity();
    let index = fixture.instances.mod_index();
    let _launch = activity.reserve_launch(fixture.id).unwrap();
    // No watcher is active yet. The launch gate still detects a removed mod.
    std::fs::remove_file(&target).unwrap();
    index
        .before_launch(&fixture.db, &activity, fixture.id, fixture.root.clone())
        .await
        .unwrap();
    let query = "SELECT presence FROM instance_content";
    let status: String = sqlx::query_scalar(query)
        .fetch_one(&fixture.db.pool)
        .await
        .unwrap();
    assert_eq!(status, "missing");
    drop(_launch);

    // The background index updates SQLite from a native notification.
    std::fs::write(&target, b"verified bytes").unwrap();
    async_std::future::timeout(Duration::from_secs(5), async {
        loop {
            let presence: String = sqlx::query_scalar(query)
                .fetch_one(&fixture.db.pool)
                .await
                .unwrap();
            if presence == "present" {
                break;
            }
            async_std::task::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .unwrap();
}
