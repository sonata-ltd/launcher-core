use super::*;
use crate::java::{managed::registrar::RuntimeRegistrar, utils::DetectedJavaRuntime};
use std::{
    os::unix::fs::PermissionsExt,
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
};

struct Registrar {
    root: PathBuf,
    operation: Uuid,
    runtimes: Mutex<Vec<Arc<JavaRuntime>>>,
    fail: AtomicBool,
    registrations: AtomicUsize,
    commits: AtomicUsize,
}

#[async_trait::async_trait]
impl RuntimeRegistrar for Registrar {
    async fn runtime(&self, id: i64) -> Result<Option<Arc<JavaRuntime>>> {
        Ok(self
            .runtimes
            .lock()
            .unwrap()
            .iter()
            .find(|runtime| runtime.id == id)
            .cloned())
    }

    async fn register_managed(&self, detected: DetectedJavaRuntime) -> Result<Arc<JavaRuntime>> {
        assert!(!crate::utils::process_pin::allows_cleanup(&self.root.join(".managed")).unwrap());
        assert_eq!(
            read_record(&self.root, self.operation)?
                .unwrap()
                .snapshot
                .stage,
            ManagedJavaStage::Validating
        );
        self.registrations.fetch_add(1, Ordering::SeqCst);
        if self.fail.load(Ordering::SeqCst) {
            return Err(ManagedJavaError::new(
                "registration",
                "fixture refused registration",
            ));
        }
        let mut runtimes = self.runtimes.lock().unwrap();
        if let Some(runtime) = runtimes.first() {
            return Ok(runtime.clone());
        }
        let runtime = Arc::new(JavaRuntime {
            id: 41,
            version: detected.version.unwrap(),
            major: detected.major.unwrap(),
            arch: detected.arch,
            exec_path: detected.exec_path.to_str().unwrap().into(),
            home_path: detected.home_path.unwrap().to_str().unwrap().into(),
            vendor: detected.vendor,
            source: JavaSource::Managed,
        });
        runtimes.push(runtime.clone());
        Ok(runtime)
    }

    fn validation_committed(&self, runtime: &JavaRuntime) {
        let snapshot = read_record(&self.root, self.operation)
            .unwrap()
            .unwrap()
            .snapshot;
        assert_eq!(snapshot.stage, ManagedJavaStage::Ready);
        assert_eq!(snapshot.runtime_id, Some(runtime.id));
        assert!(!crate::utils::process_pin::allows_cleanup(&self.root.join(".managed")).unwrap());
        self.commits.fetch_add(1, Ordering::SeqCst);
    }
}

async fn fixture() -> (tempfile::TempDir, Arc<ManagedJavaService>, Arc<Registrar>) {
    let dir = tempfile::tempdir().unwrap();
    let service = Arc::new(ManagedJavaService::new(dir.path().into()));
    service.initialize().await.unwrap();
    let mut rec = record(package(), ManagedJavaStage::AwaitingValidation);
    let id = rec.snapshot.id;
    rec.home = Some(
        PathBuf::from("managed")
            .join(&rec.snapshot.package.id)
            .join(id.to_string())
            .join("jre"),
    );
    let home = dir.path().join(rec.home.as_ref().unwrap());
    std::fs::create_dir_all(home.join("bin")).unwrap();
    let executable = extract::executable(&home);
    std::fs::write(&executable, format!("#!/bin/sh\nprintf 'java.home = {}\\njava.runtime.version = {}\\njava.specification.version = 21\\nos.arch = {}\\n'\n", home.display(), rec.snapshot.package.version, crate::java::select::host_arch())).unwrap();
    std::fs::set_permissions(executable, std::fs::Permissions::from_mode(0o700)).unwrap();
    insert(&service, rec);
    service.persist(id).await.unwrap();
    let registrar = Arc::new(Registrar {
        root: dir.path().into(),
        operation: id,
        runtimes: Mutex::new(Vec::new()),
        fail: AtomicBool::new(false),
        registrations: AtomicUsize::new(0),
        commits: AtomicUsize::new(0),
    });
    (dir, service, registrar)
}

#[async_std::test]
async fn standalone_validation_uses_only_owned_registrar_port_and_notifies_after_ready_commit() {
    let (_dir, service, registrar) = fixture().await;
    let id = registrar.operation;
    let runtime = service.validate_with(id, registrar.clone()).await.unwrap();
    assert_eq!(runtime.id, 41);
    assert_eq!(
        service
            .validate_with(id, registrar.clone())
            .await
            .unwrap()
            .id,
        41
    );
    assert_eq!(registrar.registrations.load(Ordering::SeqCst), 1);
    assert_eq!(registrar.commits.load(Ordering::SeqCst), 1);
    registrar.runtimes.lock().unwrap().clear();
    assert_eq!(
        service
            .validate_with(id, registrar.clone())
            .await
            .unwrap_err()
            .code,
        "registration"
    );
    assert_eq!(registrar.registrations.load(Ordering::SeqCst), 1);
}

#[async_std::test]
async fn registrar_failure_is_durable_without_ready_notification_and_retry_registers_once() {
    let (_dir, service, registrar) = fixture().await;
    let id = registrar.operation;
    registrar.fail.store(true, Ordering::SeqCst);
    let error = service
        .validate_with(id, registrar.clone())
        .await
        .unwrap_err();
    assert_eq!(error.code, "registration");
    assert_eq!(
        service.installation(id).await.unwrap().stage,
        ManagedJavaStage::Failed
    );
    assert_eq!(
        read_record(&service.root, id)
            .unwrap()
            .unwrap()
            .snapshot
            .error,
        Some(error)
    );
    assert_eq!(registrar.commits.load(Ordering::SeqCst), 0);
    registrar.fail.store(false, Ordering::SeqCst);
    service.validate_with(id, registrar.clone()).await.unwrap();
    assert_eq!(registrar.runtimes.lock().unwrap().len(), 1);
    assert_eq!(registrar.commits.load(Ordering::SeqCst), 1);
}

#[async_std::test]
#[allow(deprecated)]
async fn legacy_validation_call_shape_delegates_from_owning_service_module() {
    let dir = tempfile::tempdir().unwrap();
    let db = Arc::new(
        crate::data::db::Database::init("sqlite::memory:")
            .await
            .unwrap(),
    );
    let java = JavaService::new(db, crate::bus::EventBus::new(16), dir.path().into());
    let id = Uuid::new_v4();
    assert_eq!(
        java.validate_managed(id).await.unwrap_err().code,
        "not_found"
    );
}

struct PanickingRegistrar;
#[async_trait::async_trait]
impl RuntimeRegistrar for PanickingRegistrar {
    async fn runtime(&self, _: i64) -> Result<Option<Arc<JavaRuntime>>> {
        Ok(None)
    }
    async fn register_managed(&self, _: DetectedJavaRuntime) -> Result<Arc<JavaRuntime>> {
        panic!("registrar fixture");
    }
    fn validation_committed(&self, _: &JavaRuntime) {
        panic!("must not notify");
    }
}

#[async_std::test]
async fn panicking_registrar_persists_failed_and_shutdown_waits_for_failure_cleanup() {
    let (_dir, service, registrar) = fixture().await;
    let id = registrar.operation;
    assert_eq!(
        service
            .validate_with(id, Arc::new(PanickingRegistrar))
            .await
            .unwrap_err()
            .code,
        "worker_panic"
    );
    service.tasks.shutdown().await;
    assert_eq!(
        service.installation(id).await.unwrap().stage,
        ManagedJavaStage::Failed
    );
    assert_eq!(
        read_record(&service.root, id)
            .unwrap()
            .unwrap()
            .snapshot
            .stage,
        ManagedJavaStage::Failed
    );
    assert!(crate::utils::process_pin::allows_cleanup(&service.root.join(".managed")).unwrap());
}

#[async_std::test]
async fn validation_correlates_domain_uuid_with_common_terminal_history_without_persisting_session_id(
) {
    let (_dir, service, registrar) = fixture().await;
    let id = registrar.operation;
    assert!(service.job_id(id).await.unwrap().is_none());
    service.validate_with(id, registrar).await.unwrap();
    let job = service.job_id(id).await.unwrap().unwrap();
    let view = service.operations.reconcile();
    assert!(view.active.is_empty());
    assert_eq!(view.completed[0].snapshot.id, job);
    assert!(matches!(
        view.completed[0].outcome,
        crate::data::registry::operation::message::status::Outcome::Completed
    ));
    let loaded = ManagedJavaService::new(service.root.clone());
    assert!(loaded.job_id(id).await.unwrap().is_none());
    assert_eq!(
        loaded.installation(id).await.unwrap().stage,
        ManagedJavaStage::Ready
    );
}

#[async_std::test]
async fn failed_validation_has_common_failed_outcome_and_retry_has_a_distinct_job_id() {
    let (_dir, service, registrar) = fixture().await;
    let id = registrar.operation;
    registrar.fail.store(true, Ordering::SeqCst);
    service
        .validate_with(id, registrar.clone())
        .await
        .unwrap_err();
    let failed = service.job_id(id).await.unwrap().unwrap();
    registrar.fail.store(false, Ordering::SeqCst);
    service.validate_with(id, registrar).await.unwrap();
    assert_ne!(service.job_id(id).await.unwrap().unwrap(), failed);
    let view = service.operations.reconcile();
    assert!(view.active.is_empty());
    assert_eq!(view.completed.len(), 2);
    assert!(matches!(
        view.completed[0].outcome,
        crate::data::registry::operation::message::status::Outcome::Failed
    ));
    assert!(matches!(
        view.completed[1].outcome,
        crate::data::registry::operation::message::status::Outcome::Completed
    ));
}
