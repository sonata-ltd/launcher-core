use super::*;
use std::{fs, time::Duration};

type BlockingHook = (async_channel::Sender<()>, std::sync::mpsc::Receiver<()>);
static BLOCKING: std::sync::OnceLock<Mutex<HashMap<(PathBuf, &'static str), BlockingHook>>> =
    std::sync::OnceLock::new();
type AsyncHook = (
    futures::channel::oneshot::Sender<()>,
    futures::channel::oneshot::Receiver<()>,
);
static ASYNC: std::sync::OnceLock<Mutex<HashMap<(PathBuf, &'static str), AsyncHook>>> =
    std::sync::OnceLock::new();

pub(in crate::java::managed) fn checkpoint(root: &Path, phase: &'static str) {
    let hook = BLOCKING
        .get_or_init(Default::default)
        .lock()
        .unwrap()
        .remove(&(root.to_owned(), phase));
    if let Some((entered, resume)) = hook {
        entered.send_blocking(()).unwrap();
        resume.recv_timeout(Duration::from_secs(10)).unwrap();
    }
    if std::env::var_os("SONATA_TEST_MANAGED_ROOT").is_some_and(|value| Path::new(&value) == root)
        && std::env::var("SONATA_TEST_MANAGED_PHASE").ok().as_deref() == Some(phase)
    {
        std::process::exit(77);
    }
}

pub(in crate::java::managed) async fn async_checkpoint(root: &Path, phase: &'static str) {
    let hook = ASYNC
        .get_or_init(Default::default)
        .lock()
        .unwrap()
        .remove(&(root.to_owned(), phase));
    if let Some((entered, resume)) = hook {
        let _ = entered.send(());
        let _ = resume.await;
    }
}

pub(super) fn blocking_hook(
    root: &Path,
    phase: &'static str,
) -> (async_channel::Receiver<()>, std::sync::mpsc::Sender<()>) {
    let (entered, receiver) = async_channel::bounded(1);
    let (resume, wait) = std::sync::mpsc::channel();
    BLOCKING
        .get_or_init(Default::default)
        .lock()
        .unwrap()
        .insert((root.to_owned(), phase), (entered, wait));
    (receiver, resume)
}

#[async_std::test]
async fn live_legacy_unknown_and_future_active_owners_are_never_recovered_or_cancelled() {
    for stage in [
        ManagedJavaStage::Downloading,
        ManagedJavaStage::Extracting,
        ManagedJavaStage::Validating,
        ManagedJavaStage::Cancelling,
    ] {
        for mode in ["live", "legacy", "zero-pid", "future", "wrong-operation"] {
            let dir = tempfile::tempdir().unwrap();
            let service = ManagedJavaService::new(dir.path().into());
            let mut rec = record(package(), stage);
            if mode == "legacy" {
                rec.owner = None;
            } else {
                let mut owner = serde_json::to_value(rec.owner.as_ref().unwrap()).unwrap();
                match mode {
                    "zero-pid" => owner["pid"] = 0.into(),
                    "future" => owner["version"] = 99.into(),
                    "wrong-operation" => owner["operation"] = Uuid::new_v4().to_string().into(),
                    _ => {}
                }
                rec.owner = Some(serde_json::from_value(owner).unwrap());
            }
            let id = insert(&service, rec);
            fs::create_dir_all(service.staging(id)).unwrap();
            let partial = service.staging(id).join("archive.part");
            fs::write(&partial, b"keep").unwrap();
            service.persist(id).await.unwrap();
            let path = dir
                .path()
                .join(".managed/operations")
                .join(format!("{id}.json"));
            let before = fs::read(&path).unwrap();
            let loaded = ManagedJavaService::new(dir.path().into());
            assert_eq!(
                loaded.installation(id).await.unwrap().stage,
                stage,
                "{mode}"
            );
            if stage == ManagedJavaStage::Validating {
                assert_eq!(
                    loaded.cancel(id).await.unwrap(),
                    CancelDisposition::AlreadyFinished
                );
            } else {
                assert_eq!(loaded.cancel(id).await.unwrap_err().code, "busy", "{mode}");
            }
            assert_eq!(fs::read(path).unwrap(), before);
            assert_eq!(fs::read(partial).unwrap(), b"keep");
        }
    }
}

#[async_std::test]
async fn dead_operation_recovery_requires_matching_regular_versioned_staging_marker() {
    for mode in ["missing", "broken", "mismatch", "future", "oversized"] {
        let dir = tempfile::tempdir().unwrap();
        let service = ManagedJavaService::new(dir.path().into());
        let mut rec = record(package(), ManagedJavaStage::Downloading);
        rec.owner = Some(dead_owner(rec.snapshot.id));
        let id = insert(&service, rec);
        create_staging(&service, id);
        let marker = service.staging(id).join(".sonata-owner.json");
        match mode {
            "missing" => fs::remove_file(&marker).unwrap(),
            "broken" => fs::write(&marker, b"broken").unwrap(),
            "mismatch" => fs::write(
                &marker,
                serde_json::to_vec(&ownership::Owner::new(id)).unwrap(),
            )
            .unwrap(),
            "oversized" => fs::write(&marker, vec![b' '; 8192]).unwrap(),
            _ => {
                let mut owner: serde_json::Value =
                    serde_json::from_slice(&fs::read(&marker).unwrap()).unwrap();
                owner["version"] = 99.into();
                fs::write(&marker, serde_json::to_vec(&owner).unwrap()).unwrap();
            }
        }
        fs::write(service.staging(id).join("archive.part"), b"keep").unwrap();
        service.persist(id).await.unwrap();
        let path = dir
            .path()
            .join(".managed/operations")
            .join(format!("{id}.json"));
        let before = fs::read(&path).unwrap();
        let loaded = ManagedJavaService::new(dir.path().into());
        assert_eq!(
            loaded.installation(id).await.unwrap().stage,
            ManagedJavaStage::Downloading,
            "{mode}"
        );
        assert_eq!(fs::read(path).unwrap(), before);
        assert_eq!(
            fs::read(service.staging(id).join("archive.part")).unwrap(),
            b"keep"
        );
    }
}

#[async_std::test]
async fn local_failure_preserves_unknown_staging_and_persists_cleanup_diagnostic() {
    for mode in ["missing", "mismatch"] {
        let dir = tempfile::tempdir().unwrap();
        let service = ManagedJavaService::new(dir.path().into());
        service.initialize().await.unwrap();
        let id = insert(&service, record(package(), ManagedJavaStage::Downloading));
        create_staging(&service, id);
        let marker = service.staging(id).join(".sonata-owner.json");
        if mode == "missing" {
            fs::remove_file(marker).unwrap();
        } else {
            fs::write(
                marker,
                serde_json::to_vec(&ownership::Owner::new(Uuid::new_v4())).unwrap(),
            )
            .unwrap();
        }
        let error = service
            .fail(id, ManagedJavaError::new("download", "fixture failed"))
            .await;
        assert!(error.details.contains("Staging cleanup deferred"));
        assert!(service.staging(id).is_dir());
        assert_eq!(
            ManagedJavaService::new(dir.path().into())
                .installation(id)
                .await
                .unwrap()
                .error,
            Some(error)
        );
    }
}

#[test]
fn staging_creation_never_clobbers_existing_directory_or_adopts_a_foreign_owner() {
    let dir = tempfile::tempdir().unwrap();
    let id = Uuid::new_v4();
    let path = ownership::create_staging(dir.path(), id, &ownership::Owner::new(id))
        .unwrap()
        .path()
        .to_owned();
    fs::write(path.join("user-data"), b"keep").unwrap();
    assert!(ownership::create_staging(dir.path(), id, &ownership::Owner::new(id)).is_err());
    assert!(
        ownership::create_staging(dir.path(), Uuid::new_v4(), &ownership::Owner::new(id)).is_err()
    );
    assert!(ownership::cleanup_staging(dir.path(), id, &ownership::Owner::new(id), true).is_err());
    assert_eq!(fs::read(path.join("user-data")).unwrap(), b"keep");
    ownership::cleanup_staging(dir.path(), id, &ownership::Owner::new(id), false).unwrap();
    assert!(!path.exists());
}

#[async_std::test]
async fn independent_managed_writers_share_admission_but_exclude_recovery_and_duplicate_operation_writes(
) {
    let dir = tempfile::tempdir().unwrap();
    let id = Uuid::new_v4();
    let first = ownership::access(dir.path()).await.unwrap();
    let second = ownership::access(dir.path()).await.unwrap();
    let operation = ownership::operation(dir.path(), id).unwrap();
    assert_eq!(
        ownership::operation(dir.path(), id).err().unwrap().code,
        "busy"
    );
    assert!(ownership::operation(dir.path(), Uuid::new_v4()).is_ok());
    assert_eq!(
        ManagedJavaService::new(dir.path().into())
            .initialize()
            .await
            .unwrap_err()
            .code,
        "busy"
    );
    drop(first);
    assert!(ownership::maintenance(dir.path()).is_err());
    drop(second);
    drop(operation);
    let recovery = ownership::maintenance(dir.path()).unwrap();
    assert!(
        crate::utils::object_store::ObjectStore::new(dir.path().join(".managed"))
            .try_access()
            .is_err()
    );
    drop(recovery);
    ManagedJavaService::new(dir.path().into())
        .initialize()
        .await
        .unwrap();
}

#[async_std::test]
async fn validation_rechecks_durable_active_record_instead_of_claiming_stale_awaiting_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let db = Arc::new(
        crate::data::db::Database::init("sqlite::memory:")
            .await
            .unwrap(),
    );
    let java = JavaService::new(db, crate::bus::EventBus::new(16), dir.path().into());
    let mut rec = record(package(), ManagedJavaStage::AwaitingValidation);
    rec.home = Some("managed/test/jre".into());
    let service = ManagedJavaService::new(dir.path().into());
    let id = insert(&service, rec);
    service.persist(id).await.unwrap();
    java.managed.initialize().await.unwrap();
    service.update(id, |rec| rec.snapshot.stage = ManagedJavaStage::Downloading);
    service.persist(id).await.unwrap();
    let before = fs::read(
        dir.path()
            .join(".managed/operations")
            .join(format!("{id}.json")),
    )
    .unwrap();
    assert_eq!(java.validate_managed(id).await.unwrap_err().code, "busy");
    assert_eq!(java.managed.cancel(id).await.unwrap_err().code, "busy");
    assert_eq!(
        fs::read(
            dir.path()
                .join(".managed/operations")
                .join(format!("{id}.json"))
        )
        .unwrap(),
        before
    );
    assert!(java.list_all().await.unwrap().is_empty());
}

#[async_std::test]
async fn dropping_start_caller_keeps_record_and_install_worker_owned_until_cancelled_cleanup_finishes(
) {
    let dir = tempfile::tempdir().unwrap();
    let service = Arc::new(ManagedJavaService::new(dir.path().into()));
    service.initialize().await.unwrap();
    let (entered, receiver) = futures::channel::oneshot::channel();
    let (resume, wait) = futures::channel::oneshot::channel();
    ASYNC
        .get_or_init(Default::default)
        .lock()
        .unwrap()
        .insert((dir.path().into(), "started"), (entered, wait));
    let (staged, release_staging) = blocking_hook(dir.path(), "staging");
    let mut caller = Box::pin(service.start_package(package()));
    match futures::future::select(receiver, caller.as_mut()).await {
        futures::future::Either::Left((result, _)) => result.unwrap(),
        futures::future::Either::Right((result, _)) => panic!("start finished early: {result:?}"),
    }
    drop(caller);
    let id = service
        .entries
        .lock()
        .unwrap()
        .keys()
        .copied()
        .next()
        .unwrap();
    assert!(ownership::maintenance(dir.path()).is_err());
    assert!(ownership::operation(dir.path(), id).is_err());
    resume.send(()).unwrap();
    staged.recv().await.unwrap();
    assert!(ownership::maintenance(dir.path()).is_err());
    service.cancel(id).await.unwrap();
    release_staging.send(()).unwrap();
    let done = crate::utils::object_store::ObjectStore::new(dir.path().join(".managed"))
        .maintenance(&Cancellation::new())
        .await
        .unwrap();
    drop(done);
    assert_eq!(
        read_record(dir.path(), id).unwrap().unwrap().snapshot.stage,
        ManagedJavaStage::Cancelled
    );
    assert!(!service.staging(id).exists());
    assert!(ownership::operation(dir.path(), id).is_ok());
}

#[async_std::test]
async fn dropping_recovery_caller_retains_exclusive_admission_through_blocking_cleanup_and_record_commit(
) {
    let dir = tempfile::tempdir().unwrap();
    let service = ManagedJavaService::new(dir.path().into());
    let mut rec = record(package(), ManagedJavaStage::Extracting);
    rec.owner = Some(dead_owner(rec.snapshot.id));
    let id = insert(&service, rec);
    create_staging(&service, id);
    service.persist(id).await.unwrap();
    let (entered, resume) = blocking_hook(dir.path(), "cleanup");
    let recovery = ManagedJavaService::new(dir.path().into());
    let mut caller = Box::pin(recovery.initialize());
    match futures::future::select(Box::pin(entered.recv()), caller.as_mut()).await {
        futures::future::Either::Left((result, _)) => result.unwrap(),
        futures::future::Either::Right((result, _)) => {
            panic!("recovery finished early: {result:?}")
        }
    }
    drop(caller);
    assert_eq!(
        ManagedJavaService::new(dir.path().into())
            .initialize()
            .await
            .unwrap_err()
            .code,
        "busy"
    );
    assert!(
        crate::utils::object_store::ObjectStore::new(dir.path().join(".managed"))
            .try_access()
            .is_err()
    );
    resume.send(()).unwrap();
    let access = ownership::access(dir.path()).await.unwrap();
    drop(access);
    assert!(!service.staging(id).exists());
    assert_eq!(
        read_record(dir.path(), id).unwrap().unwrap().snapshot.stage,
        ManagedJavaStage::Failed
    );
}

#[cfg(unix)]
#[async_std::test]
async fn dropped_validation_caller_keeps_broad_pin_after_child_exit_until_ready_record_is_committed(
) {
    use crate::utils::process_pin::allows_cleanup;
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let db = Arc::new(
        crate::data::db::Database::init("sqlite::memory:")
            .await
            .unwrap(),
    );
    let java = JavaService::new(db, crate::bus::EventBus::new(16), dir.path().into());
    java.managed.initialize().await.unwrap();
    let mut rec = record(package(), ManagedJavaStage::AwaitingValidation);
    let id = rec.snapshot.id;
    let home = PathBuf::from("managed")
        .join(&rec.snapshot.package.id)
        .join(id.to_string())
        .join("jre");
    rec.home = Some(home.clone());
    let home = dir.path().join(home);
    fs::create_dir_all(home.join("bin")).unwrap();
    let executable = home.join("bin/java");
    fs::write(&executable, format!("#!/bin/sh\nprintf 'java.home = {}\\njava.runtime.version = {}\\njava.specification.version = 21\\nos.arch = {}\\njava.vendor = fixture\\n'\n", home.display(), rec.snapshot.package.version, crate::java::select::host_arch())).unwrap();
    fs::set_permissions(executable, fs::Permissions::from_mode(0o700)).unwrap();
    insert(&java.managed, rec);
    java.managed.persist(id).await.unwrap();
    let (entered, resume) = blocking_hook(dir.path(), "validation-ready");
    let mut caller = Box::pin(java.validate_managed(id));
    match futures::future::select(Box::pin(entered.recv()), caller.as_mut()).await {
        futures::future::Either::Left((result, _)) => result.unwrap(),
        futures::future::Either::Right((result, _)) => {
            panic!("validation finished early: {result:?}")
        }
    }
    drop(caller);
    let scope = dir.path().join(".managed");
    assert!(!allows_cleanup(&scope).unwrap());
    let pins: Vec<_> = fs::read_dir(scope.join(".sonata-processes"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    assert_eq!(pins.len(), 2);
    assert_eq!(
        pins.iter()
            .filter(|path| path.join(".sonata-exited").exists())
            .count(),
        1
    );
    assert!(ownership::maintenance(dir.path()).is_err());
    assert!(ownership::operation(dir.path(), id).is_err());
    assert_eq!(java.list_all().await.unwrap().len(), 1);
    resume.send(()).unwrap();
    drop(
        crate::utils::object_store::ObjectStore::new(scope.clone())
            .maintenance(&Cancellation::new())
            .await
            .unwrap(),
    );
    assert!(allows_cleanup(&scope).unwrap());
    assert_eq!(
        read_record(dir.path(), id).unwrap().unwrap().snapshot.stage,
        ManagedJavaStage::Ready
    );
}

#[async_std::test]
async fn managed_operation_crash_writer() {
    let Some(root) = std::env::var_os("SONATA_TEST_MANAGED_ROOT").map(PathBuf::from) else {
        return;
    };
    let service = ManagedJavaService::new(root.clone());
    service.initialize().await.unwrap();
    let _admission = ownership::access(&root).await.unwrap();
    let source = root.join("fixture.tar.gz");
    tar_fixture(&source);
    let package = pin_archive(package(), &source);
    let rec = record(package.clone(), ManagedJavaStage::Downloading);
    let id = insert(&service, rec);
    let _operation = ownership::operation(&root, id).unwrap();
    service.persist(id).await.unwrap();
    create_staging(&service, id);
    let archive = service.staging(id).join("archive.part");
    fs::copy(source, &archive).unwrap();
    service
        .install_archive(
            id,
            &package,
            &Cancellation::new(),
            archive,
            package.sha256.clone(),
        )
        .await
        .unwrap();
    panic!("missed managed crash boundary");
}

#[async_std::test]
async fn process_exit_before_and_after_managed_publication_replays_only_dead_owned_staging() {
    for phase in [
        "created",
        "staging",
        "archive-imported",
        "home-owned",
        "published",
        "cleanup",
    ] {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_owned();
        let phase_arg = phase.to_owned();
        let output = task::spawn_blocking(move || {
            std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "java::managed::tests::ownership_tests::managed_operation_crash_writer",
                    "--nocapture",
                ])
                .env("SONATA_TEST_MANAGED_ROOT", root)
                .env("SONATA_TEST_MANAGED_PHASE", phase_arg)
                .output()
                .unwrap()
        })
        .await;
        assert_eq!(
            output.status.code(),
            Some(77),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(ownership::maintenance(dir.path()).is_ok());
        let recovery = ManagedJavaService::new(dir.path().into());
        let snapshots = recovery.installations().await.unwrap();
        assert_eq!(snapshots.len(), 1);
        let snapshot = &snapshots[0];
        assert_eq!(
            snapshot.stage,
            if phase == "created" {
                ManagedJavaStage::Downloading
            } else if ["staging", "archive-imported", "home-owned"].contains(&phase) {
                ManagedJavaStage::Failed
            } else {
                ManagedJavaStage::AwaitingValidation
            }
        );
        assert_eq!(recovery.staging(snapshot.id).exists(), phase == "created");
        if ["published", "cleanup"].contains(&phase) {
            let rec = read_record(dir.path(), snapshot.id).unwrap().unwrap();
            assert!(extract::executable(&dir.path().join(rec.home.unwrap())).is_file());
        }
        assert_eq!(
            ManagedJavaService::new(dir.path().into())
                .installation(snapshot.id)
                .await
                .unwrap(),
            *snapshot
        );
    }
}

#[cfg(unix)]
#[async_std::test]
async fn managed_root_aliases_share_admission_and_escaping_links_never_modify_external_files() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("java");
    fs::create_dir(&root).unwrap();
    let alias = dir.path().join("alias");
    std::os::unix::fs::symlink(&root, &alias).unwrap();
    let writer = ownership::access(&root).await.unwrap();
    assert_eq!(ownership::maintenance(&alias).err().unwrap().code, "busy");
    drop(writer);
    for location in [".managed/staging", ".managed/operations", "managed"] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("java");
        let outside = temp.path().join("outside");
        fs::create_dir(&root).unwrap();
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("victim"), b"keep").unwrap();
        let link = root.join(location);
        fs::create_dir_all(link.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&outside, &link).unwrap();
        let service = ManagedJavaService::new(root.clone());
        let mut rec = record(package(), ManagedJavaStage::Downloading);
        rec.owner = Some(dead_owner(rec.snapshot.id));
        let id = insert(&service, rec);
        if location.ends_with("operations") {
            assert!(service.persist(id).await.is_err());
            assert!(service.initialize().await.is_err());
        } else if location.ends_with("staging") {
            assert!(ownership::create_staging(&root, id, &ownership::Owner::new(id)).is_err());
            assert!(service.cleanup_staging(id).await.is_err());
        } else {
            service.update(id, |rec| rec.home = Some("managed/jre".into()));
            service.persist(id).await.unwrap();
            let recovered = ManagedJavaService::new(root.clone());
            let report = recovered.installations_report().await.unwrap();
            assert!(report.installations.is_empty());
            assert_eq!(report.diagnostics.len(), 1);
            assert_eq!(report.diagnostics[0].operation, Some(id));
        }
        assert_eq!(fs::read(outside.join("victim")).unwrap(), b"keep");
        assert_eq!(fs::read_dir(outside).unwrap().count(), 1);
    }
}

#[cfg(unix)]
#[async_std::test]
async fn validation_rejects_escaping_executable_before_spawning_it() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let db = Arc::new(
        crate::data::db::Database::init("sqlite::memory:")
            .await
            .unwrap(),
    );
    let java = JavaService::new(db, crate::bus::EventBus::new(16), dir.path().join("java"));
    java.managed.initialize().await.unwrap();
    let mut rec = record(package(), ManagedJavaStage::AwaitingValidation);
    rec.home = Some("managed/jre".into());
    let home = java.managed.root.join(rec.home.as_ref().unwrap());
    fs::create_dir_all(home.join("bin")).unwrap();
    let ran = dir.path().join("ran");
    let external = dir.path().join("external-java");
    fs::write(&external, format!("#!/bin/sh\ntouch '{}'\n", ran.display())).unwrap();
    fs::set_permissions(&external, fs::Permissions::from_mode(0o755)).unwrap();
    std::os::unix::fs::symlink(&external, extract::executable(&home)).unwrap();
    let id = insert(&java.managed, rec);
    assert_eq!(java.validate_managed(id).await.unwrap_err().code, "storage");
    assert!(!ran.exists());
    assert!(java.list_all().await.unwrap().is_empty());
}

#[async_std::test]
async fn recovery_rejects_record_identity_mismatch_and_unsafe_home_without_modifying_bytes() {
    for mode in ["identity", "parent", "absolute", "backslash"] {
        let dir = tempfile::tempdir().unwrap();
        let service = ManagedJavaService::new(dir.path().into());
        let mut rec = record(package(), ManagedJavaStage::Downloading);
        rec.owner = Some(dead_owner(rec.snapshot.id));
        rec.home = match mode {
            "parent" => Some("../outside".into()),
            "absolute" => Some(dir.path().join("outside")),
            "backslash" => Some("managed\\unsafe".into()),
            _ => None,
        };
        let id = insert(&service, rec);
        create_staging(&service, id);
        service.persist(id).await.unwrap();
        let mut path = dir
            .path()
            .join(".managed/operations")
            .join(format!("{id}.json"));
        if mode == "identity" {
            let other = path.with_file_name(format!("{}.json", Uuid::new_v4()));
            fs::rename(&path, &other).unwrap();
            path = other;
        }
        let bytes = fs::read(&path).unwrap();
        let recovered = ManagedJavaService::new(dir.path().into());
        let report = recovered.installations_report().await.unwrap();
        assert!(report.installations.is_empty(), "{mode}");
        assert_eq!(report.diagnostics.len(), 1, "{mode}");
        assert_eq!(report.diagnostics[0].path, path);
        assert_eq!(fs::read(path).unwrap(), bytes);
        assert!(service.staging(id).is_dir());
    }
}

#[async_std::test]
async fn managed_publication_never_replaces_a_preexisting_destination() {
    let dir = tempfile::tempdir().unwrap();
    let service = ManagedJavaService::new(dir.path().into());
    service.initialize().await.unwrap();
    let source = dir.path().join("fixture.tar.gz");
    tar_fixture(&source);
    let package = pin_archive(package(), &source);
    let id = insert(
        &service,
        record(package.clone(), ManagedJavaStage::Downloading),
    );
    create_staging(&service, id);
    let archive = service.staging(id).join("archive.part");
    fs::copy(source, &archive).unwrap();
    let destination = dir
        .path()
        .join("managed")
        .join(&package.id)
        .join(id.to_string());
    fs::create_dir_all(&destination).unwrap();
    fs::write(destination.join("user-data"), b"keep").unwrap();
    let error = service
        .install_archive(
            id,
            &package,
            &Cancellation::new(),
            archive,
            package.sha256.clone(),
        )
        .await
        .unwrap_err();
    assert!(error.details.contains("already exists"));
    service.fail(id, error).await;
    assert_eq!(fs::read(destination.join("user-data")).unwrap(), b"keep");
    assert_eq!(fs::read_dir(destination).unwrap().count(), 1);
}

#[async_std::test]
async fn publication_preserves_a_replacement_generation_with_or_without_cancellation() {
    for cancel_after_publish in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let service = Arc::new(ManagedJavaService::new(dir.path().into()));
        service.initialize().await.unwrap();
        let source = dir.path().join("fixture.tar.gz");
        tar_fixture(&source);
        let package = pin_archive(package(), &source);
        let id = insert(
            &service,
            record(package.clone(), ManagedJavaStage::Downloading),
        );
        create_staging(&service, id);
        let archive = service.staging(id).join("archive.part");
        fs::copy(&source, &archive).unwrap();
        let destination = dir
            .path()
            .join("managed")
            .join(&package.id)
            .join(id.to_string());
        let previous = dir.path().join("previous");
        let (arrived, resume) = blocking_hook(dir.path(), "published");
        let cancel = Cancellation::new();
        let worker_cancel = cancel.clone();
        let worker = service.clone();
        let install = task::spawn(async move {
            worker
                .install_archive(
                    id,
                    &package,
                    &worker_cancel,
                    archive,
                    package.sha256.clone(),
                )
                .await
        });
        async_std::future::timeout(Duration::from_secs(10), arrived.recv())
            .await
            .unwrap()
            .unwrap();
        fs::rename(&destination, &previous).unwrap();
        fs::create_dir(&destination).unwrap();
        fs::write(destination.join("user-data"), b"preserve these bytes").unwrap();
        if cancel_after_publish {
            cancel.cancel();
        }
        resume.send(()).unwrap();

        let error = install.await.unwrap_err();

        assert_eq!(
            error.code,
            if cancel_after_publish {
                ManagedJavaErrorKind::Cancelled
            } else {
                ManagedJavaErrorKind::Storage
            }
        );
        assert_eq!(
            fs::read(destination.join("user-data")).unwrap(),
            b"preserve these bytes"
        );
        assert!(extract::executable(&previous.join("jre")).is_file());
    }
}

#[async_std::test]
async fn managed_live_writer_process() {
    let Some(root) = std::env::var_os("SONATA_TEST_MANAGED_LIVE").map(PathBuf::from) else {
        return;
    };
    let service = ManagedJavaService::new(root.clone());
    service.initialize().await.unwrap();
    let admission = ownership::access(&root).await.unwrap();
    let id = insert(&service, record(package(), ManagedJavaStage::Downloading));
    let operation = ownership::operation(&root, id).unwrap();
    service.persist(id).await.unwrap();
    create_staging(&service, id);
    fs::write(service.staging(id).join("archive.part"), b"partial").unwrap();
    println!("MANAGED_LOCKED");
    std::io::stdout().flush().unwrap();
    let mut line = String::new();
    std::io::stdin().read_line(&mut line).unwrap();
    drop(operation);
    drop(admission);
    println!("MANAGED_RELEASED");
    std::io::stdout().flush().unwrap();
    line.clear();
    std::io::stdin().read_line(&mut line).unwrap();
}

#[async_std::test]
async fn independent_process_admission_and_live_owner_both_gate_recovery_until_proven_exit() {
    use async_std::io::{prelude::BufReadExt, BufReader};
    use futures::AsyncWriteExt;
    let dir = tempfile::tempdir().unwrap();
    let mut command = async_std::process::Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--exact",
            "java::managed::tests::ownership_tests::managed_live_writer_process",
            "--nocapture",
        ])
        .env("SONATA_TEST_MANAGED_LIVE", dir.path())
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    let mut child = command.spawn().unwrap();
    let mut input = child.stdin.take().unwrap();
    let mut output = BufReader::new(child.stdout.take().unwrap());
    async fn until(reader: &mut BufReader<async_std::process::ChildStdout>, marker: &str) {
        async_std::future::timeout(Duration::from_secs(5), async {
            let mut line = String::new();
            loop {
                line.clear();
                assert!(
                    reader.read_line(&mut line).await.unwrap() > 0,
                    "missing {marker}"
                );
                if line.contains(marker) {
                    break;
                }
            }
        })
        .await
        .unwrap();
    }
    until(&mut output, "MANAGED_LOCKED").await;
    assert_eq!(
        ManagedJavaService::new(dir.path().into())
            .initialize()
            .await
            .unwrap_err()
            .code,
        "busy"
    );
    input.write_all(b"release\n").await.unwrap();
    input.flush().await.unwrap();
    until(&mut output, "MANAGED_RELEASED").await;
    let live = ManagedJavaService::new(dir.path().into());
    let snapshot = live.installations().await.unwrap().remove(0);
    assert_eq!(snapshot.stage, ManagedJavaStage::Downloading);
    assert!(live.staging(snapshot.id).exists());
    assert_eq!(live.cancel(snapshot.id).await.unwrap_err().code, "busy");
    input.write_all(b"exit\n").await.unwrap();
    input.flush().await.unwrap();
    assert!(child.status().await.unwrap().success());
    let dead = ManagedJavaService::new(dir.path().into());
    assert_eq!(
        dead.installation(snapshot.id).await.unwrap().stage,
        ManagedJavaStage::Failed
    );
    assert!(!dead.staging(snapshot.id).exists());
}
