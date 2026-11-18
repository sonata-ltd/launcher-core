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
