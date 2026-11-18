use super::*;
use crate::utils::object_store::ObjectStore;
use std::fs;

fn fixture() -> (tempfile::TempDir, PathBuf, PathBuf, ManagedJavaPackage) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("java");
    fs::create_dir(&root).unwrap();
    let source = dir.path().join("archive.tar.gz");
    tar_fixture(&source);
    let package = pin_archive(package(), &source);
    (dir, root, source, package)
}

#[async_std::test]
async fn archive_bytes_reuse_authoritative_sha256_across_paths_packages_and_relocation() {
    let (dir, root, source, package) = fixture();
    let cancel = Cancellation::new();
    let first = bytes::import(&root, source.clone(), &package, &cancel)
        .await
        .unwrap();
    let original = fs::read(&first.path).unwrap();
    let path = first.path.clone();
    drop(first);
    let other_source = dir.path().join("other-archive");
    fs::copy(source, &other_source).unwrap();
    let mut other = package.clone();
    other.id = "another-package-role".into();
    other.url = "https://different.invalid/archive".into();
    let second = bytes::import(&root, other_source, &other, &cancel)
        .await
        .unwrap();
    assert_eq!(second.path, path);
    drop(second);
    let moved = dir.path().join("moved-java");
    fs::rename(root, &moved).unwrap();
    let cached = bytes::cached(&moved, &other, &cancel)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(fs::read(cached.path).unwrap(), original);
}

#[async_std::test]
async fn extracted_managed_homes_are_independent_placements_of_the_same_immutable_archive() {
    let (_dir, root, source, package) = fixture();
    let service = ManagedJavaService::new(root.clone());
    service.initialize().await.unwrap();
    let mut homes = Vec::new();
    let original = fs::read(&source).unwrap();
    for name in ["first-role", "second-role"] {
        let mut package = package.clone();
        package.id = name.into();
        let id = insert(
            &service,
            record(package.clone(), ManagedJavaStage::Downloading),
        );
        create_staging(&service, id);
        let archive = service.staging(id).join("archive.part");
        fs::copy(&source, &archive).unwrap();
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
        let home = root.join(
            service
                .entries
                .lock()
                .unwrap()
                .get(&id)
                .unwrap()
                .record
                .home
                .as_ref()
                .unwrap(),
        );
        homes.push(home);
    }
    fs::write(extract::executable(&homes[0]), b"modified home").unwrap();
    assert_eq!(
        fs::read(extract::executable(&homes[1])).unwrap(),
        b"placeholder"
    );
    let cached = bytes::cached(&root, &package, &Cancellation::new())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(fs::read(cached.path).unwrap(), original);
}

#[async_std::test]
async fn production_install_uses_verified_cached_archive_without_transport() {
    let (_dir, root, source, package) = fixture();
    let service = Arc::new(ManagedJavaService::new(root.clone()));
    service.initialize().await.unwrap();
    drop(
        bytes::import(&root, source, &package, &Cancellation::new())
            .await
            .unwrap(),
    );
    let id = service.start_package(package.clone()).await.unwrap();
    let done = ObjectStore::new(root.join(".managed"))
        .maintenance(&Cancellation::new())
        .await
        .unwrap();
    drop(done);
    let snapshot = service.installation(id).await.unwrap();
    assert_eq!(
        snapshot.stage,
        ManagedJavaStage::AwaitingValidation,
        "{:?}",
        snapshot.error
    );
    assert_eq!(snapshot.downloaded_bytes, package.size);
    assert!(!service.staging(id).exists());
}

#[async_std::test]
async fn claimed_transport_digest_or_size_is_never_substituted_for_actual_archive_bytes() {
    for mode in ["digest", "size"] {
        let (_dir, root, source, mut package) = fixture();
        if mode == "digest" {
            package.sha256 = "ab".repeat(32);
        } else {
            package.size += 1;
        }
        let service = ManagedJavaService::new(root.clone());
        service.initialize().await.unwrap();
        let id = insert(
            &service,
            record(package.clone(), ManagedJavaStage::Downloading),
        );
        create_staging(&service, id);
        let archive = service.staging(id).join("archive.part");
        fs::copy(source, &archive).unwrap();
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
        assert_eq!(error.code, "checksum", "{mode}");
        let error = service.fail(id, error).await;
        assert_eq!(error.stage, Some(ManagedJavaStage::VerifyingChecksum));
        assert!(!root.join("managed").exists());
        assert!(!root.join(".managed/bytes/objects").exists());
    }
}

#[async_std::test]
async fn corrupt_canonical_archive_is_not_a_hit_or_overwritten_and_cancelled_reads_do_not_import() {
    let (_dir, root, source, package) = fixture();
    let object = bytes::import(&root, source.clone(), &package, &Cancellation::new())
        .await
        .unwrap();
    let mut wrong_size = package.clone();
    wrong_size.size += 1;
    assert!(bytes::cached(&root, &wrong_size, &Cancellation::new())
        .await
        .unwrap()
        .is_none());
    fs::write(&object.path, b"corrupt canonical archive").unwrap();
    let path = object.path.clone();
    drop(object);
    assert!(bytes::cached(&root, &package, &Cancellation::new())
        .await
        .unwrap()
        .is_none());
    assert!(
        bytes::import(&root, source.clone(), &package, &Cancellation::new())
            .await
            .is_err()
    );
    assert_eq!(fs::read(path).unwrap(), b"corrupt canonical archive");
    let cancel = Cancellation::new();
    cancel.cancel();
    assert_eq!(
        bytes::cached(&root, &package, &cancel)
            .await
            .err()
            .unwrap()
            .code,
        "cancelled"
    );
    assert_eq!(
        bytes::import(&root, source, &package, &cancel)
            .await
            .err()
            .unwrap()
            .code,
        "cancelled"
    );
}

#[async_std::test]
async fn dropping_archive_import_caller_retains_worker_admission_until_verified_publication_finishes(
) {
    let (_dir, root, source, package) = fixture();
    let cancel = Cancellation::new();
    let (entered, resume) = ownership_tests::blocking_hook(&root, "archive-imported");
    let mut caller = Box::pin(bytes::import(&root, source, &package, &cancel));
    match futures::future::select(Box::pin(entered.recv()), caller.as_mut()).await {
        futures::future::Either::Left((result, _)) => result.unwrap(),
        futures::future::Either::Right((_, _)) => panic!("import finished early"),
    };
    drop(caller);
    let store = ObjectStore::new(root.join(".managed/bytes"));
    assert!(store.try_maintenance().is_err());
    resume.send(()).unwrap();
    let done = store.maintenance(&Cancellation::new()).await.unwrap();
    drop(done);
    assert!(store
        .find_sha256(&package.sha256, &Cancellation::new())
        .unwrap()
        .is_some());
}

#[async_std::test]
async fn managed_startup_sweeps_only_dead_owned_archive_import_scratch_and_preserves_canonical_bytes(
) {
    let (_dir, root, source, package) = fixture();
    let object = bytes::import(&root, source, &package, &Cancellation::new())
        .await
        .unwrap();
    let object_path = object.path.clone();
    drop(object);
    let scratch = root.join(".managed/bytes/staging");
    let owner = serde_json::to_value(dead_owner(Uuid::new_v4())).unwrap();
    let pid = owner["pid"].as_u64().unwrap();
    for name in ["dead", "live", "unknown"] {
        let path = scratch.join(format!("import-{name}"));
        fs::create_dir(&path).unwrap();
        fs::write(path.join("partial"), b"keep").unwrap();
        if name != "unknown" {
            fs::write(
                path.join(".sonata-owner"),
                if name == "dead" {
                    pid.to_string()
                } else {
                    std::process::id().to_string()
                },
            )
            .unwrap();
        }
    }
    ManagedJavaService::new(root).initialize().await.unwrap();
    assert!(!scratch.join("import-dead").exists());
    assert!(scratch.join("import-live").is_dir() && scratch.join("import-unknown").is_dir());
    assert!(object_path.is_file());
}

#[async_std::test]
async fn dropping_extract_caller_keeps_cas_admission_in_blocking_worker() {
    let (_dir, root, source, package) = fixture();
    let service = ManagedJavaService::new(root.clone());
    service.initialize().await.unwrap();
    let id = insert(
        &service,
        record(package.clone(), ManagedJavaStage::Downloading),
    );
    create_staging(&service, id);
    let archive = service.staging(id).join("archive.part");
    fs::copy(source, &archive).unwrap();
    let (entered, resume) = ownership_tests::blocking_hook(&root, "extracting");
    let cancel = Cancellation::new();
    let mut caller =
        Box::pin(service.install_archive(id, &package, &cancel, archive, package.sha256.clone()));
    match futures::future::select(Box::pin(entered.recv()), caller.as_mut()).await {
        futures::future::Either::Left((result, _)) => result.unwrap(),
        futures::future::Either::Right((result, _)) => panic!("finished early: {result:?}"),
    };
    drop(caller);
    let cas = ObjectStore::new(root.join(".managed/bytes"));
    assert!(cas.try_maintenance().is_err());
    resume.send(()).unwrap();
    let done = cas.maintenance(&Cancellation::new()).await.unwrap();
    drop(done);
    assert!(cas
        .find_sha256(&package.sha256, &Cancellation::new())
        .unwrap()
        .is_some());
    // The test dropped a low-level install future, not an owned production job;
    // only the blocking extraction completes. Its staging is still retained.
    assert!(service.staging(id).join("contents").is_dir());
}

#[cfg(unix)]
#[async_std::test]
async fn archive_cas_escaping_roots_shards_and_scratch_never_modify_external_bytes() {
    for mode in ["root", "objects", "shard", "staging", "aliases"] {
        let (dir, root, source, package) = fixture();
        let outside = dir.path().join("outside");
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("victim"), b"keep").unwrap();
        let path = match mode {
            "root" => root.join(".managed/bytes"),
            "objects" => root.join(".managed/bytes/objects/sha256"),
            "shard" => root
                .join(".managed/bytes/objects/sha256")
                .join(&package.sha256[..2]),
            "staging" => root.join(".managed/bytes/staging"),
            _ => root.join(".managed/bytes/aliases/sha1"),
        };
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&outside, path).unwrap();
        assert!(
            bytes::import(&root, source, &package, &Cancellation::new())
                .await
                .is_err(),
            "{mode}"
        );
        assert!(
            bytes::cached(&root, &package, &Cancellation::new())
                .await
                .is_err(),
            "{mode}"
        );
        assert_eq!(fs::read(outside.join("victim")).unwrap(), b"keep");
        assert_eq!(fs::read_dir(outside).unwrap().count(), 1);
    }
}

#[async_std::test]
async fn direct_sha256_lookup_is_verified_and_maintenance_excludes_it_before_reading_bytes() {
    let (_dir, root, source, package) = fixture();
    let object = bytes::import(&root, source, &package, &Cancellation::new())
        .await
        .unwrap();
    let path = object.path.clone();
    drop(object);
    let store = ObjectStore::new(root.join(".managed/bytes"));
    let maintenance = store.try_maintenance().unwrap();
    assert_eq!(
        store
            .find_sha256(&package.sha256, &Cancellation::new())
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::WouldBlock
    );
    drop(maintenance);
    assert!(store
        .find_sha256(&package.sha256.to_uppercase(), &Cancellation::new())
        .unwrap()
        .is_some());
    fs::write(path, b"changed").unwrap();
    assert!(store
        .find_sha256(&package.sha256, &Cancellation::new())
        .unwrap()
        .is_none());
    assert!(store
        .find_sha256("../../outside", &Cancellation::new())
        .is_err());
}
