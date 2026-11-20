//! Real CAS/SQLite operations at deterministic admission boundaries; no network.
use super::*;
use crate::instance::content::{install, install::checkpoints::Pause};
use futures::{pin_mut, poll};
use std::{io, task::Poll};

fn assert_busy(store: &ObjectStore) {
    assert_eq!(
        store.try_maintenance().err().unwrap().kind(),
        io::ErrorKind::WouldBlock
    );
}

#[async_std::test]
async fn bootstrap_recovery_owns_lifetime_locks_and_finishes_sql_after_caller_drop() {
    let fixture = fixture().await;
    let (operation, _) = prepared(&fixture).await;
    let store = ObjectStore::new(fixture.paths.root().join("content-store"));
    let mut pause = Pause::new(operation, "recovering");
    let mut bootstrap = Box::pin(InstanceContentService::new(
        fixture.db.clone(),
        fixture.paths.clone(),
        fixture.instances.clone(),
        None,
        fixture.operations.clone(),
    ));
    assert!(matches!(
        futures::future::select(bootstrap.as_mut(), Box::pin(pause.entered())).await,
        futures::future::Either::Right(_)
    ));
    drop(bootstrap);
    assert_eq!(
        store.lock_exclusive().err().unwrap().kind(),
        io::ErrorKind::WouldBlock
    );
    assert!(storage::has_pending(&fixture.db, fixture.id).await.unwrap());
    let stage_exists = std::fs::read_dir(&fixture.root).unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".sonata-recovery-")
    });
    assert!(stage_exists);
    pause.resume();
    async_std::future::timeout(std::time::Duration::from_secs(10), async {
        while storage::has_pending(&fixture.db, fixture.id).await.unwrap() {
            async_std::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let bindings: i64 = sqlx::query_scalar("SELECT count(*) FROM instance_content")
        .fetch_one(&fixture.db.pool)
        .await
        .unwrap();
    assert_eq!(bindings, 1);
    assert_eq!(
        std::fs::read(fixture.root.join("mods/mod.jar")).unwrap(),
        b"verified bytes"
    );
}

#[async_std::test]
async fn imported_unregistered_bytes_stay_admitted_until_sql_lease_registration() {
    let fixture = fixture().await;
    let store = ObjectStore::new(fixture.paths.root().join("content-store"));
    let access = store.try_access().unwrap();
    let operation = uuid::Uuid::new_v4();
    storage::begin_preparation(
        &fixture.db,
        operation,
        fixture.id,
        &access.root_key().unwrap(),
    )
    .await
    .unwrap();
    let source = fixture.root.join("fresh.jar");
    std::fs::write(&source, b"fresh unregistered bytes").unwrap();
    let hash = crate::utils::files::sha256_file(&source).unwrap();
    let mut pause = Pause::new(operation, "imported");
    let cancel = Cancellation::new();
    let import =
        install::import_and_register(&fixture.db, access, source, operation, &cancel, |_| Ok(()));
    let inspect = async {
        pause.entered().await;
        assert_eq!(
            std::fs::read(store.path_for(&hash).unwrap()).unwrap(),
            b"fresh unregistered bytes"
        );
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM content_objects WHERE sha256=?")
            .bind(&hash)
            .fetch_one(&fixture.db.pool)
            .await
            .unwrap();
        assert_eq!(count, 0);
        assert!(storage::leased_objects(&fixture.db, operation)
            .await
            .unwrap()
            .is_empty());
        assert_busy(&store);
        pause.resume();
    };
    let (imported, ()) = futures::join!(import, inspect);
    let (object, id) = imported.unwrap();
    assert_eq!(object.digests.sha256, hash);
    assert_eq!(
        storage::leased_objects(&fixture.db, operation)
            .await
            .unwrap()
            .get(&id),
        Some(&hash)
    );
    // Exclusive admission alone does NOT authorize collection of this preparation.
    assert!(store.try_maintenance().is_ok());
    assert!(storage::has_pending(&fixture.db, fixture.id).await.unwrap());
}

#[async_std::test]
async fn installation_keeps_admission_through_publication_and_sql_completion() {
    let fixture = fixture().await;
    let (store, _, release) = cached_release(&fixture).await;
    let signature = crate::instance::content::inventory::signature(fixture.root.clone())
        .await
        .unwrap();
    let operation = uuid::Uuid::new_v4();
    let mut pause = Pause::new(operation, "completing");
    let cancel = Cancellation::new();
    let releases = [release];
    let execute = install::execute(
        fixture.db.clone(),
        store.clone(),
        fixture.id,
        &fixture.root,
        operation,
        &releases,
        &signature,
        &cancel,
        |_, _| assert_busy(&store),
    );
    let inspect = async {
        pause.entered().await;
        assert_eq!(
            std::fs::read(fixture.root.join("mods/cached.jar")).unwrap(),
            b"cached mod"
        );
        assert!(!storage::operation_completed(&fixture.db, operation)
            .await
            .unwrap());
        assert_eq!(
            storage::leased_objects(&fixture.db, operation)
                .await
                .unwrap()
                .len(),
            1
        );
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM instance_content")
            .fetch_one(&fixture.db.pool)
            .await
            .unwrap();
        assert_eq!(count, 0);
        assert_busy(&store);
        pause.resume();
    };
    let (result, ()) = futures::join!(execute, inspect);
    result.unwrap();
    assert!(storage::operation_completed(&fixture.db, operation)
        .await
        .unwrap());
    assert!(storage::leased_objects(&fixture.db, operation)
        .await
        .unwrap()
        .is_empty());
    assert!(store.try_maintenance().is_ok());
}

#[async_std::test]
async fn recovery_keeps_admission_until_completed_bindings_replace_journal_leases() {
    let fixture = fixture().await;
    let (operation, _) = prepared(&fixture).await;
    let store = ObjectStore::new(fixture.paths.root().join("content-store"));
    let mut pause = Pause::new(operation, "recovering");
    let registry = fixture.instances.get_registry();
    let recovery = recovery::recover(&fixture.db, &fixture.paths, &registry, None);
    let inspect = async {
        pause.entered().await;
        assert_eq!(
            std::fs::read(fixture.root.join("mods/mod.jar")).unwrap(),
            b"verified bytes"
        );
        assert_eq!(
            storage::leased_objects(&fixture.db, operation)
                .await
                .unwrap()
                .len(),
            1
        );
        assert!(!storage::operation_completed(&fixture.db, operation)
            .await
            .unwrap());
        assert_busy(&store);
        pause.resume();
    };
    let (result, ()) = futures::join!(recovery, inspect);
    assert!(result.unwrap().is_empty());
    assert!(storage::operation_completed(&fixture.db, operation)
        .await
        .unwrap());
    assert!(store.try_maintenance().is_ok());
}

#[async_std::test]
async fn cancelling_admission_wait_does_not_create_a_preparation_or_staging() {
    let fixture = fixture().await;
    let (store, _, release) = cached_release(&fixture).await;
    let maintenance = store.try_maintenance().unwrap();
    let signature = crate::instance::content::inventory::signature(fixture.root.clone())
        .await
        .unwrap();
    let releases = [release];
    let cancel = Cancellation::new();
    let operation = uuid::Uuid::new_v4();
    let execute = install::execute(
        fixture.db.clone(),
        store.clone(),
        fixture.id,
        &fixture.root,
        operation,
        &releases,
        &signature,
        &cancel,
        |_, _| panic!("must wait before preparing"),
    );
    pin_mut!(execute);
    assert!(matches!(poll!(execute.as_mut()), Poll::Pending));
    assert!(!storage::has_pending(&fixture.db, fixture.id).await.unwrap());
    cancel.cancel();
    assert!(matches!(execute.await, Err(error) if error.is_cancelled()));
    assert!(!storage::has_pending(&fixture.db, fixture.id).await.unwrap());
    assert!(content_staging_paths(&fixture.root).is_empty());
    assert!(!fixture.root.join("mods/cached.jar").exists());
    drop(maintenance);
    assert!(store.try_access().is_ok());
}

#[async_std::test]
async fn startup_cannot_discard_preparations_while_a_reader_holds_admission() {
    let fixture = fixture().await;
    let store = ObjectStore::new(fixture.paths.root().join("content-store"));
    let access = store.try_access().unwrap();
    let operation = uuid::Uuid::new_v4();
    storage::begin_preparation(
        &fixture.db,
        operation,
        fixture.id,
        &access.root_key().unwrap(),
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
    assert!(storage::has_pending(&fixture.db, fixture.id).await.unwrap());
    drop(access);
    let _service = InstanceContentService::new(
        fixture.db.clone(),
        fixture.paths.clone(),
        fixture.instances.clone(),
        None,
        fixture.operations.clone(),
    )
    .await
    .unwrap();
    assert!(!storage::has_pending(&fixture.db, fixture.id).await.unwrap());
}

#[async_std::test]
async fn dropping_bootstrap_does_not_release_admission_before_the_sql_cleanup_finishes() {
    let fixture = fixture().await;
    let store = ObjectStore::new(fixture.paths.root().join("content-store"));
    let operation = uuid::Uuid::new_v4();
    storage::begin_preparation(
        &fixture.db,
        operation,
        fixture.id,
        &store.root_key().unwrap(),
    )
    .await
    .unwrap();
    let mut pause = Pause::new(operation, "cleanup");
    let mut bootstrap = Box::pin(InstanceContentService::new(
        fixture.db.clone(),
        fixture.paths.clone(),
        fixture.instances.clone(),
        None,
        fixture.operations.clone(),
    ));
    assert!(poll!(bootstrap.as_mut()).is_pending());
    pause.entered().await;
    drop(bootstrap);
    assert_busy(&store);
    assert!(storage::has_pending(&fixture.db, fixture.id).await.unwrap());
    pause.resume();
    let cancel = Cancellation::new();
    let _access =
        async_std::future::timeout(std::time::Duration::from_secs(10), store.access(&cancel))
            .await
            .unwrap()
            .unwrap();
    assert!(!storage::has_pending(&fixture.db, fixture.id).await.unwrap());
}
