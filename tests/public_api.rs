//! A real external consumer, not a unit test with crate-private access.
use sonata_launcher_core::{
    instance::{
        error::{InitError, InstanceError, ManifestError},
        settings::SettingsPatch,
    },
    java::managed::{ManagedInstallationsReport, ManagedJavaErrorKind},
    operations::{message::OperationId, JobId, OperationReconciliation, TaskResult},
    Config, DbError, GlobalState,
};
use std::path::Path;

fn isolated_config(root: &Path) -> Config {
    Config {
        root_override: Some(root.to_owned()),
        discover_java_on_startup: false,
        ..Config::init()
    }
}

#[async_std::test]
async fn ownership_exported_instance_service_keeps_lifetime_locks() {
    let root = tempfile::tempdir().unwrap();
    let core = GlobalState::new(isolated_config(root.path()))
        .await
        .unwrap();
    core.shutdown().await;
    let instances = core.instances();
    drop(core);
    assert!(GlobalState::new(isolated_config(root.path()))
        .await
        .is_err());
    assert!(instances.list().await.unwrap().is_empty());
    drop(instances);
    let reopened = GlobalState::new(isolated_config(root.path()))
        .await
        .unwrap();
    reopened.shutdown().await;
}

#[async_std::test]
async fn ownership_shared_sqlite_rejects_distinct_layout_in_another_process_and_after_restart() {
    const CHILD: &str = "SONATA_TEST_FOREIGN_ROOT_BOOTSTRAP";
    if let Some(root) = std::env::var_os(CHILD) {
        let root = std::path::PathBuf::from(root);
        let mut config = isolated_config(&root.join("foreign"));
        config.dirs.cache_db = Some(root.join("shared.sqlite"));
        assert!(matches!(
            GlobalState::new(config).await,
            Err(sonata_launcher_core::AppError::Database(
                DbError::LayoutConflict
            ))
        ));
        assert!(!root.join("foreign/assets").exists());
        return;
    }

    let root = tempfile::tempdir().unwrap();
    let mut config = isolated_config(&root.path().join("owner"));
    config.database_url = Some(format!(
        "sqlite://{}",
        root.path().join("shared.sqlite").display()
    ));
    let core = GlobalState::new(config).await.unwrap();
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .current_dir(root.path())
        .args([
            "--exact",
            "ownership_shared_sqlite_rejects_distinct_layout_in_another_process_and_after_restart",
            "--nocapture",
        ])
        .env(CHILD, root.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    core.shutdown().await;
    drop(core);
    let mut config = isolated_config(&root.path().join("foreign"));
    config.dirs.cache_db = Some(root.path().join("shared.sqlite"));
    assert!(matches!(
        GlobalState::new(config).await,
        Err(sonata_launcher_core::AppError::Database(
            DbError::LayoutConflict
        ))
    ));
    assert!(!root.path().join("foreign/assets").exists());
}

#[async_std::test]
async fn ownership_coincident_launcher_and_instance_roots_open_once() {
    let root = tempfile::tempdir().unwrap();
    let mut config = isolated_config(root.path());
    config.dirs.instances = Some(root.path().join("."));
    let core = GlobalState::new(config).await.unwrap();
    core.shutdown().await;
}

#[async_std::test]
async fn ownership_shutdown_rejects_instance_and_options_mutators() {
    let root = tempfile::tempdir().unwrap();
    let core = GlobalState::new(isolated_config(root.path()))
        .await
        .unwrap();
    let instances = core.instances();
    let create = |name: &str| {
        instances.create(
            name.to_owned(),
            "1.20.1".into(),
            "https://example.invalid/manifest".into(),
            "prism".into(),
        )
    };
    let instance = create("Before shutdown").await.unwrap();
    core.shutdown().await;
    assert!(matches!(
        create("After shutdown").await,
        Err(InstanceError::Task(_))
    ));
    assert!(matches!(
        core.instances().rename(instance.id, "renamed").await,
        Err(InstanceError::Task(_))
    ));
    assert!(matches!(
        core.instances().unregister(instance.id).await,
        Err(InstanceError::Task(_))
    ));
    assert!(matches!(
        core.instances()
            .update_settings(instance.id, &SettingsPatch::default())
            .await,
        Err(InstanceError::Task(_))
    ));
    assert!(core
        .options()
        .patch_settings(instance.id, Default::default())
        .await
        .is_err());
    assert_eq!(core.instances().list().await.unwrap().len(), 1);
}

#[async_std::test]
async fn isolated_consumer_uses_facades_reports_and_cooperative_shutdown() {
    let root = tempfile::tempdir().unwrap();
    let core = GlobalState::new(Config {
        root_override: Some(root.path().into()),
        discover_java_on_startup: false,
        ..Config::init()
    })
    .await
    .unwrap();
    // Subscribe before reconciliation: this is the public observer boundary.
    let _events = core.bus.subscribe();
    let operations: OperationReconciliation = core.operations();
    assert!(operations.active.is_empty());
    let instance = core
        .instances()
        .create(
            "API fixture".into(),
            "1.20.1".into(),
            "https://example.invalid/manifest".into(),
            "prism".into(),
        )
        .await
        .unwrap();
    core.instances()
        .update_settings(
            instance.id,
            &SettingsPatch {
                memory_min: Some(Some(512)),
                memory_max: Some(Some(1024)),
                ..SettingsPatch::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(
        core.instances()
            .settings(instance.id)
            .await
            .unwrap()
            .memory_max,
        Some(1024)
    );
    assert!(core
        .content()
        .pending_recovery(instance.id)
        .await
        .unwrap()
        .is_empty());
    let report: ManagedInstallationsReport =
        core.java().managed.installations_report().await.unwrap();
    assert!(report.installations.is_empty());
    assert!(report.diagnostics.is_empty());
    let _: Vec<TaskResult> = core.shutdown().await;
    assert!(matches!(
        core.instances().install(instance.id).await,
        Err(InstanceError::Task(_))
    ));
    // Error contracts remain nameable without exposing mutation capabilities.
    let _: InstanceError =
        InitError::Manifest(ManifestError::ManifestMalformed("fixture".into())).into();
    let _: InstanceError = DbError::ResultCorrupted.into();
    let unknown = ManagedJavaErrorKind::Unknown("future_domain_code".into());
    assert_eq!(unknown.as_str(), "future_domain_code");
    fn same_identity(id: JobId) -> OperationId {
        id
    }
    let _ = same_identity;
}
