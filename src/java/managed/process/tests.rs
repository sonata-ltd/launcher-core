use super::*;
use crate::utils::process_pin::allows_cleanup;

async fn fixture() -> (tempfile::TempDir, JavaRetention) {
    let dir = tempfile::tempdir().unwrap();
    let database = Arc::new(
        Database::init(&format!(
            "sqlite://{}",
            dir.path().join("owner.db").display()
        ))
        .await
        .unwrap(),
    );
    let retention = JavaRetention::new(database, dir.path().join("java"));
    (dir, retention)
}

#[async_std::test]
async fn retention_precedes_reads_and_drop_is_not_completion() {
    let (_dir, retention) = fixture().await;
    let java_use = retention.begin(&Cancellation::new()).await.unwrap();
    let scope = retention.root.join(".managed");
    assert!(scope.join(".sonata-roots.json").is_file());
    assert!(!allows_cleanup(&scope).unwrap());
    assert!(ObjectStore::new(scope.clone()).try_maintenance().is_err());
    drop(java_use);
    assert!(ObjectStore::new(scope.clone()).try_maintenance().is_ok());
    assert!(!allows_cleanup(&scope).unwrap());
}

#[async_std::test]
async fn cancelled_retention_does_not_create_a_root_or_pin() {
    let (_dir, retention) = fixture().await;
    let cancel = Cancellation::new();
    cancel.cancel();
    assert_eq!(
        retention.begin(&cancel).await.err().unwrap().kind(),
        io::ErrorKind::Interrupted
    );
    assert!(!retention.root.exists());
}

#[async_std::test]
async fn broad_completion_cannot_release_independent_unknown_child_intent() {
    let (_dir, retention) = fixture().await;
    let java_use = retention.begin(&Cancellation::new()).await.unwrap();
    let child = java_use.child_pin().unwrap();
    child.before_spawn().unwrap();
    java_use.finish(true);
    let scope = retention.root.join(".managed");
    assert!(!allows_cleanup(&scope).unwrap());
    child.finish(false);
    assert!(!allows_cleanup(&scope).unwrap());
    child.finish(true);
    assert!(allows_cleanup(&scope).unwrap());
}

#[async_std::test]
async fn owned_reader_clone_keeps_admission_after_caller_drop_until_worker_completion() {
    let (_dir, retention) = fixture().await;
    let java_use = retention.begin(&Cancellation::new()).await.unwrap();
    let pin = java_use.pin();
    let worker = java_use.clone();
    let (entered, ready) = futures::channel::oneshot::channel();
    let (resume, wait) = std::sync::mpsc::channel();
    let task = async_std::task::spawn_blocking(move || {
        let _reader = worker;
        entered.send(()).unwrap();
        wait.recv().unwrap();
    });
    ready.await.unwrap();
    drop(java_use);
    let scope = retention.root.join(".managed");
    assert!(ObjectStore::new(scope.clone()).try_maintenance().is_err());
    resume.send(()).unwrap();
    task.await;
    assert!(ObjectStore::new(scope.clone()).try_maintenance().is_ok());
    assert!(!allows_cleanup(&scope).unwrap());
    pin.finish(true);
    assert!(allows_cleanup(&scope).unwrap());
}
