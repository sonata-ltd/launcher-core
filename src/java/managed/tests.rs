use super::*;
use futures::io::Cursor;
use sha2::{Digest, Sha256};
use std::io::Write;

fn package() -> ManagedJavaPackage {
    ManagedJavaPackage {
        id: "temurin-test".into(),
        version: "21.0.5+11".into(),
        major: 21,
        target: JavaTarget::host().unwrap(),
        image_type: "jre".into(),
        url: "https://example.invalid/java.tar.gz".into(),
        size: 3,
        sha256: format!("{:x}", Sha256::digest(b"abc")),
        format: ArchiveFormat::TarGz,
    }
}

fn record(package: ManagedJavaPackage, stage: ManagedJavaStage) -> Record {
    Record {
        snapshot: ManagedJavaSnapshot {
            id: Uuid::new_v4(),
            created_at: chrono::Utc::now(),
            package,
            stage,
            downloaded_bytes: 0,
            total_bytes: Some(3),
            runtime_id: None,
            error: None,
        },
        home: None,
    }
}

fn insert(service: &ManagedJavaService, record: Record) -> Uuid {
    let id = record.snapshot.id;
    service.entries.lock().unwrap().insert(
        id,
        Entry {
            record,
            cancel: Cancellation::new(),
        },
    );
    id
}

fn assets() -> serde_json::Value {
    serde_json::json!([{
        "binary": {
            "architecture": "x64", "os": "linux", "image_type": "jre",
            "package": { "name": "java.tar.gz", "link": "https://example.invalid/java.tar.gz", "size": 123, "checksum": "ab".repeat(32) }
        },
        "version": { "major": 21, "openjdk_version": "21.0.5+11" }
    }])
}

#[test]
fn catalog_filters_platform_and_rejects_invalid_integrity_metadata() {
    let target = JavaTarget {
        os: "linux".into(),
        arch: "x64".into(),
    };
    assert_eq!(catalog::parse(assets(), &target, 21).unwrap().len(), 1);
    assert!(catalog::parse(assets(), &target, 17).unwrap().is_empty());
    let other = JavaTarget {
        os: "mac".into(),
        arch: "aarch64".into(),
    };
    assert!(catalog::parse(assets(), &other, 21).unwrap().is_empty());
    let mut invalid = assets();
    invalid[0]["binary"]["package"]["checksum"] = "bad".into();
    assert_eq!(
        catalog::parse(invalid, &target, 21).unwrap_err().code,
        "catalog"
    );
}

#[test]
fn catalog_identity_is_stable_and_distinguishes_platforms() {
    let target = JavaTarget {
        os: "linux".into(),
        arch: "x64".into(),
    };
    let first = catalog::parse(assets(), &target, 21).unwrap().remove(0);
    assert_eq!(
        first.id,
        catalog::parse(assets(), &target, 21).unwrap()[0].id
    );
    let mut value = assets();
    value[0]["binary"]["package"]["checksum"] = "cd".repeat(32).into();
    assert_ne!(first.id, catalog::parse(value, &target, 21).unwrap()[0].id);
}

#[async_std::test]
async fn streamed_download_counts_written_bytes_and_checks_length() {
    let bytes = vec![42u8; 150_000];
    let mut reader = Cursor::new(bytes.clone());
    let mut writer = Cursor::new(Vec::new());
    let progress = Mutex::new(Vec::new());
    let digest = transfer::stream_body(
        &mut reader,
        &mut writer,
        bytes.len() as u64,
        &Cancellation::new(),
        |n| progress.lock().unwrap().push(n),
    )
    .await
    .unwrap();
    assert_eq!(writer.into_inner(), bytes);
    assert_eq!(digest, format!("{:x}", Sha256::digest(&bytes)));
    let progress = progress.into_inner().unwrap();
    assert!(progress.len() >= 3);
    assert_eq!(progress.last(), Some(&(bytes.len() as u64)));
    assert!(progress.windows(2).all(|w| w[0] < w[1]));
    let error = transfer::stream_body(
        &mut Cursor::new(b"ab"),
        &mut Cursor::new(Vec::new()),
        3,
        &Cancellation::new(),
        |_| {},
    )
    .await
    .unwrap_err();
    assert!(error.details.contains("received 2"));
    let error = transfer::stream_body(
        &mut Cursor::new(b"abcd"),
        &mut Cursor::new(Vec::new()),
        3,
        &Cancellation::new(),
        |_| {},
    )
    .await
    .unwrap_err();
    assert!(error.details.contains("more than expected"));
}

#[async_std::test]
async fn cancellation_interrupts_a_pending_network_read() {
    struct Pending;
    impl futures::AsyncRead for Pending {
        fn poll_read(
            self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
            _: &mut [u8],
        ) -> std::task::Poll<std::io::Result<usize>> {
            std::task::Poll::Pending
        }
    }
    let cancel = Cancellation::new();
    let worker_cancel = cancel.clone();
    let worker = task::spawn(async move {
        transfer::stream_body(
            &mut Pending,
            &mut Cursor::new(Vec::new()),
            3,
            &worker_cancel,
            |_| {},
        )
        .await
    });
    task::sleep(std::time::Duration::from_millis(10)).await;
    cancel.cancel();
    let result = async_std::future::timeout(std::time::Duration::from_secs(1), worker)
        .await
        .unwrap();
    assert_eq!(result.unwrap_err().code, "cancelled");
}

#[async_std::test]
async fn write_errors_reach_the_caller_without_false_progress() {
    struct Broken;
    impl futures::AsyncWrite for Broken {
        fn poll_write(
            self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
            _: &[u8],
        ) -> std::task::Poll<std::io::Result<usize>> {
            std::task::Poll::Ready(Err(std::io::Error::other("disk full")))
        }
        fn poll_flush(
            self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            std::task::Poll::Ready(Ok(()))
        }
        fn poll_close(
            self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            std::task::Poll::Ready(Ok(()))
        }
    }
    let error = transfer::stream_body(
        &mut Cursor::new(b"abc"),
        &mut Broken,
        3,
        &Cancellation::new(),
        |_| panic!("unwritten bytes must not count"),
    )
    .await
    .unwrap_err();
    assert!(error.details.contains("disk full"));
}

fn tar_fixture(path: &Path) {
    let encoder = flate2::write::GzEncoder::new(
        std::fs::File::create(path).unwrap(),
        flate2::Compression::default(),
    );
    let mut tar = tar::Builder::new(encoder);
    let mut header = tar::Header::new_gnu();
    let content = b"placeholder";
    header.set_size(content.len() as u64);
    header.set_mode(0o755);
    header.set_cksum();
    tar.append_data(
        &mut header,
        format!(
            "jre/bin/{}",
            if cfg!(windows) { "java.exe" } else { "java" }
        ),
        &content[..],
    )
    .unwrap();
    tar.into_inner().unwrap().finish().unwrap();
}

#[test]
fn tar_extraction_finds_java_and_preserves_executable_permission() {
    let dir = tempfile::tempdir().unwrap();
    let archive = dir.path().join("java.tar.gz");
    tar_fixture(&archive);
    let home = extract::extract(
        &archive,
        &dir.path().join("contents"),
        ArchiveFormat::TarGz,
        &Cancellation::new(),
    )
    .unwrap();
    assert!(extract::executable(&home).is_file());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(extract::executable(&home))
                .unwrap()
                .permissions()
                .mode()
                & 0o111,
            0o111
        );
    }
}

#[test]
fn zip_extraction_rejects_traversal() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("java.zip");
    let mut zip = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
    zip.start_file("../outside", zip::write::SimpleFileOptions::default())
        .unwrap();
    zip.write_all(b"escape").unwrap();
    zip.finish().unwrap();
    assert!(extract::extract(
        &path,
        &dir.path().join("contents"),
        ArchiveFormat::Zip,
        &Cancellation::new()
    )
    .is_err());
    assert!(!dir.path().join("outside").exists());
}

#[cfg(unix)]
#[test]
fn tar_extraction_rejects_links_outside_destination() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("java.tar.gz");
    let encoder = flate2::write::GzEncoder::new(
        std::fs::File::create(&path).unwrap(),
        flate2::Compression::default(),
    );
    let mut tar = tar::Builder::new(encoder);
    let mut header = tar::Header::new_gnu();
    header.set_size(0);
    header.set_entry_type(tar::EntryType::Symlink);
    header.set_mode(0o777);
    header.set_cksum();
    tar.append_link(&mut header, "jre/escape", "../../outside")
        .unwrap();
    tar.into_inner().unwrap().finish().unwrap();
    assert!(extract::extract(
        &path,
        &dir.path().join("contents"),
        ArchiveFormat::TarGz,
        &Cancellation::new()
    )
    .is_err());
}

#[test]
fn cancelled_extraction_does_not_create_java_home() {
    let dir = tempfile::tempdir().unwrap();
    let archive = dir.path().join("java.tar.gz");
    tar_fixture(&archive);
    let cancel = Cancellation::new();
    cancel.cancel();
    let result = extract::extract(
        &archive,
        &dir.path().join("contents"),
        ArchiveFormat::TarGz,
        &cancel,
    );
    assert_eq!(result.unwrap_err().code, "cancelled");
}

#[async_std::test]
async fn checksum_failure_is_persisted_and_partial_files_are_removed() {
    let dir = tempfile::tempdir().unwrap();
    let service = ManagedJavaService::new(dir.path().into());
    service.initialize().await.unwrap();
    let package = package();
    let id = insert(
        &service,
        record(package.clone(), ManagedJavaStage::Downloading),
    );
    std::fs::create_dir_all(service.staging(id)).unwrap();
    let error = service
        .install_archive(
            id,
            &package,
            &Cancellation::new(),
            service.staging(id).join("archive.part"),
            "wrong".into(),
        )
        .await
        .unwrap_err();
    service.fail(id, error).await;
    let snapshot = service.installation(id).await.unwrap();
    assert_eq!(snapshot.stage, ManagedJavaStage::Failed);
    assert_eq!(
        snapshot.error.as_ref().unwrap().stage,
        Some(ManagedJavaStage::VerifyingChecksum)
    );
    assert!(!service.staging(id).exists());
    let recovered = ManagedJavaService::new(dir.path().into())
        .installation(id)
        .await
        .unwrap();
    assert!(recovered
        .error
        .unwrap()
        .details
        .contains("Expected SHA-256"));
}

#[async_std::test]
async fn installation_waits_for_explicit_validation_and_survives_restart() {
    let dir = tempfile::tempdir().unwrap();
    let service = ManagedJavaService::new(dir.path().into());
    service.initialize().await.unwrap();
    let package = package();
    let id = insert(
        &service,
        record(package.clone(), ManagedJavaStage::Downloading),
    );
    std::fs::create_dir_all(service.staging(id)).unwrap();
    let archive = service.staging(id).join("archive.part");
    tar_fixture(&archive);
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
    assert_eq!(
        service.installation(id).await.unwrap().stage,
        ManagedJavaStage::AwaitingValidation
    );
    assert_eq!(
        service.cancel(id).await.unwrap(),
        CancelDisposition::AlreadyFinished
    );
    let recovered = ManagedJavaService::new(dir.path().into())
        .installation(id)
        .await
        .unwrap();
    assert_eq!(recovered.stage, ManagedJavaStage::AwaitingValidation);
    assert_eq!(recovered.runtime_id, None);
}

#[async_std::test]
async fn interrupted_download_is_recovered_as_failure_and_cleaned() {
    let dir = tempfile::tempdir().unwrap();
    let service = ManagedJavaService::new(dir.path().into());
    let id = insert(&service, record(package(), ManagedJavaStage::Downloading));
    std::fs::create_dir_all(service.staging(id)).unwrap();
    std::fs::write(service.staging(id).join("archive.part"), b"partial").unwrap();
    service.persist(id).await.unwrap();
    let recovered = ManagedJavaService::new(dir.path().into())
        .installation(id)
        .await
        .unwrap();
    assert_eq!(recovered.stage, ManagedJavaStage::Failed);
    assert_eq!(recovered.error.unwrap().code, "interrupted");
    assert!(!service.staging(id).exists());
}

#[async_std::test]
async fn duplicate_package_install_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let service = Arc::new(ManagedJavaService::new(dir.path().into()));
    insert(
        &service,
        record(package(), ManagedJavaStage::AwaitingValidation),
    );
    assert_eq!(
        service.start_package(package()).await.unwrap_err().code,
        "busy"
    );
}

#[async_std::test]
async fn cancellation_is_idempotent_and_finishes_after_cleanup() {
    let dir = tempfile::tempdir().unwrap();
    let service = ManagedJavaService::new(dir.path().into());
    service.initialize().await.unwrap();
    let id = insert(&service, record(package(), ManagedJavaStage::Downloading));
    assert_eq!(
        service.cancel(id).await.unwrap(),
        CancelDisposition::Requested
    );
    assert_eq!(
        service.cancel(id).await.unwrap(),
        CancelDisposition::Requested
    );
    service
        .fail(id, ManagedJavaError::new("cancelled", "Cancelled"))
        .await;
    assert_eq!(
        service.cancel(id).await.unwrap(),
        CancelDisposition::AlreadyFinished
    );
    assert_eq!(
        service.installation(id).await.unwrap().stage,
        ManagedJavaStage::Cancelled
    );
}

#[cfg(unix)]
#[async_std::test]
async fn validation_registers_managed_java_once_and_retains_failed_process_output() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let db = Arc::new(
        crate::data::db::Database::init("sqlite::memory:")
            .await
            .unwrap(),
    );
    let java = JavaService::new(db, crate::bus::EventBus::new(16), dir.path().into());
    java.managed.initialize().await.unwrap();
    let package = package();
    let mut rec = record(package.clone(), ManagedJavaStage::AwaitingValidation);
    rec.home = Some(PathBuf::from("managed/test/jre"));
    let home = dir.path().join(rec.home.as_ref().unwrap());
    std::fs::create_dir_all(home.join("bin")).unwrap();
    let exec = extract::executable(&home);
    std::fs::write(&exec, "#!/bin/sh\necho 'broken runtime' >&2\nexit 7\n").unwrap();
    std::fs::set_permissions(&exec, std::fs::Permissions::from_mode(0o755)).unwrap();
    let id = insert(&java.managed, rec);
    let error = java.managed.validate(id, &java).await.unwrap_err();
    assert!(error.details.contains("broken runtime"));
    assert!(error.details.contains('7'));
    assert_eq!(error.stage, Some(ManagedJavaStage::Validating));
    assert_eq!(
        java.managed.installation(id).await.unwrap().error,
        Some(error.clone())
    );
    let recovered = ManagedJavaService::new(dir.path().into())
        .installation(id)
        .await
        .unwrap();
    assert_eq!(recovered.error, Some(error));
    assert!(java.list_all().await.unwrap().is_empty());
    let arch = crate::java::select::host_arch();
    std::fs::write(&exec, format!("#!/bin/sh\ncat >&2 <<'JAVA'\njava.home = {}\njava.runtime.version = {}\njava.specification.version = 21\nos.arch = {arch}\njava.vendor = Test\nJAVA\n", home.display(), package.version)).unwrap();
    let runtime = java.managed.validate(id, &java).await.unwrap();
    assert_eq!(runtime.source, JavaSource::Managed);
    assert_eq!(
        java.managed.validate(id, &java).await.unwrap().id,
        runtime.id
    );
    assert_eq!(java.list_all().await.unwrap().len(), 1);
    assert_eq!(
        java.managed.installation(id).await.unwrap().stage,
        ManagedJavaStage::Ready
    );
}

#[test]
fn discovery_excludes_managed_and_staging_directories() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("java");
    for name in ["managed", ".managed", "external"] {
        let home = root.join(name);
        std::fs::create_dir_all(home.join("bin")).unwrap();
        std::fs::write(extract::executable(&home), b"placeholder").unwrap();
    }
    let cx = crate::java::discovery::DiscoveryContext {
        root: dir.path().into(),
        home: None,
        own_runtimes: root.clone(),
        env: |_| None,
    };
    assert_eq!(
        crate::java::discovery::candidates(&cx),
        vec![dunce::canonicalize(extract::executable(&root.join("external"))).unwrap()]
    );
}

#[test]
fn validation_rejects_wrong_major_missing_version_architecture_and_java_home() {
    use crate::java::utils::DetectedJavaRuntime;
    let dir = tempfile::tempdir().unwrap();
    let home = dunce::canonicalize(dir.path()).unwrap();
    let package = package();
    let mut detected = DetectedJavaRuntime {
        version: Some(package.version.clone()),
        major: Some(package.major),
        arch: Some(crate::java::select::host_arch().into()),
        exec_path: extract::executable(&home),
        home_path: Some(home.clone()),
        vendor: None,
    };
    assert!(validate_detected(&home, &package, &detected).is_ok());
    detected.major = Some(17);
    assert!(validate_detected(&home, &package, &detected).is_err());
    detected.major = Some(package.major);
    for version in [
        None,
        Some("".into()),
        Some("unknown".into()),
        Some("17.0.1+1".into()),
    ] {
        detected.version = version;
        assert!(validate_detected(&home, &package, &detected).is_err());
    }
    detected.version = Some(package.version.clone());
    detected.arch = Some("foreign_arch".into());
    assert!(validate_detected(&home, &package, &detected).is_err());
    detected.arch = Some(crate::java::select::host_arch().into());
    detected.home_path = Some(home.parent().unwrap().into());
    assert!(validate_detected(&home, &package, &detected).is_err());
}

#[async_std::test]
async fn interrupted_commit_recovers_installed_files_for_validation() {
    let dir = tempfile::tempdir().unwrap();
    let service = ManagedJavaService::new(dir.path().into());
    let mut rec = record(package(), ManagedJavaStage::Extracting);
    rec.home = Some(PathBuf::from("managed/committed/jre"));
    let home = dir.path().join(rec.home.as_ref().unwrap());
    std::fs::create_dir_all(home.join("bin")).unwrap();
    std::fs::write(extract::executable(&home), b"java").unwrap();
    let id = insert(&service, rec);
    service.persist(id).await.unwrap();
    let recovered = ManagedJavaService::new(dir.path().into())
        .installation(id)
        .await
        .unwrap();
    assert_eq!(recovered.stage, ManagedJavaStage::AwaitingValidation);
    assert!(recovered.runtime_id.is_none());
}

#[test]
fn zip_extraction_finds_windows_style_java_home() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("java.zip");
    let mut zip = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
    let binary = format!(
        "jre/bin/{}",
        if cfg!(windows) { "java.exe" } else { "java" }
    );
    zip.start_file(binary, zip::write::SimpleFileOptions::default())
        .unwrap();
    zip.write_all(b"java").unwrap();
    zip.finish().unwrap();
    let home = extract::extract(
        &path,
        &dir.path().join("contents"),
        ArchiveFormat::Zip,
        &Cancellation::new(),
    )
    .unwrap();
    assert!(extract::executable(&home).is_file());
}

#[test]
fn tar_extraction_finds_macos_bundle_home() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("java.tar.gz");
    let encoder = flate2::write::GzEncoder::new(
        std::fs::File::create(&path).unwrap(),
        flate2::Compression::default(),
    );
    let mut tar = tar::Builder::new(encoder);
    let mut header = tar::Header::new_gnu();
    header.set_size(4);
    header.set_mode(0o755);
    header.set_cksum();
    let binary = format!(
        "jre/Contents/Home/bin/{}",
        if cfg!(windows) { "java.exe" } else { "java" }
    );
    tar.append_data(&mut header, binary, &b"java"[..]).unwrap();
    tar.into_inner().unwrap().finish().unwrap();
    let home = extract::extract(
        &path,
        &dir.path().join("contents"),
        ArchiveFormat::TarGz,
        &Cancellation::new(),
    )
    .unwrap();
    assert!(home.ends_with("jre/Contents/Home"));
}

#[cfg(unix)]
#[async_std::test]
async fn offline_installation_flow_keeps_validation_running_after_caller_cancellation() {
    let dir = tempfile::tempdir().unwrap();
    let db = Arc::new(
        crate::data::db::Database::init("sqlite::memory:")
            .await
            .unwrap(),
    );
    let java = Arc::new(JavaService::new(
        db,
        crate::bus::EventBus::new(16),
        dir.path().into(),
    ));
    java.managed.initialize().await.unwrap();
    let mut package = package();
    let id = Uuid::new_v4();
    let final_home = dir
        .path()
        .join("managed")
        .join(&package.id)
        .join(id.to_string())
        .join("jre");
    let release = dir.path().join("release-probe");
    let script = format!("#!/bin/sh\nwhile [ ! -e '{}' ]; do sleep 0.01; done\ncat >&2 <<'JAVA'\njava.home = {}\njava.runtime.version = {}\njava.specification.version = 21\nos.arch = {}\njava.vendor = Test\nJAVA\n", release.display(), final_home.display(), package.version, crate::java::select::host_arch());
    let encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    let mut tar = tar::Builder::new(encoder);
    let mut header = tar::Header::new_gnu();
    header.set_size(script.len() as u64);
    header.set_mode(0o755);
    header.set_cksum();
    tar.append_data(&mut header, "jre/bin/java", script.as_bytes())
        .unwrap();
    let bytes = tar.into_inner().unwrap().finish().unwrap();
    package.size = bytes.len() as u64;
    package.sha256 = format!("{:x}", Sha256::digest(&bytes));
    let mut rec = record(package.clone(), ManagedJavaStage::Downloading);
    rec.snapshot.id = id;
    rec.snapshot.total_bytes = Some(package.size);
    insert(&java.managed, rec);
    let staging = java.managed.staging(id);
    fs::create_dir_all(&staging).await.unwrap();
    let archive = staging.join("archive.part");
    let mut output = fs::File::create(&archive).await.unwrap();
    let cancel = Cancellation::new();
    let digest = transfer::stream_body(
        &mut Cursor::new(bytes),
        &mut output,
        package.size,
        &cancel,
        |bytes| {
            java.managed
                .update(id, |record| record.snapshot.downloaded_bytes = bytes);
        },
    )
    .await
    .unwrap();
    drop(output);
    java.managed
        .install_archive(id, &package, &cancel, archive, digest)
        .await
        .unwrap();
    assert!(java.list_all().await.unwrap().is_empty());
    assert_eq!(
        java.managed.installation(id).await.unwrap().stage,
        ManagedJavaStage::AwaitingValidation
    );
    let worker_java = Arc::clone(&java);
    let (abort, registration) = futures::future::AbortHandle::new_pair();
    let caller = task::spawn(futures::future::Abortable::new(
        async move { worker_java.managed.validate(id, &worker_java).await },
        registration,
    ));
    async_std::future::timeout(std::time::Duration::from_secs(2), async {
        while java.managed.installation(id).await.unwrap().stage != ManagedJavaStage::Validating {
            task::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    abort.abort();
    assert!(caller.await.is_err());
    fs::write(release, b"continue").await.unwrap();
    async_std::future::timeout(std::time::Duration::from_secs(2), async {
        while java.managed.installation(id).await.unwrap().stage == ManagedJavaStage::Validating {
            task::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    let ready = java.managed.installation(id).await.unwrap();
    assert_eq!(ready.stage, ManagedJavaStage::Ready, "{:?}", ready.error);
    let registered = java.list_all().await.unwrap();
    assert_eq!(registered.len(), 1);
    assert_eq!(registered[0].source, JavaSource::Managed);
    let recovered = ManagedJavaService::new(dir.path().into())
        .installation(id)
        .await
        .unwrap();
    assert_eq!(recovered.runtime_id, Some(registered[0].id));
}

#[test]
fn validation_accepts_runtime_build_components_missing_from_catalog_version() {
    use crate::java::utils::DetectedJavaRuntime;
    let dir = tempfile::tempdir().unwrap();
    let home = dunce::canonicalize(dir.path()).unwrap();
    let mut package = package();
    package.major = 27;
    package.version = "27.0.1.0".into();
    package.target.arch = "x64".into();
    let detected = DetectedJavaRuntime {
        version: Some("27.0.1.0.2+1".into()),
        major: Some(27),
        arch: Some("amd64".into()),
        exec_path: extract::executable(&home),
        home_path: Some(home.clone()),
        vendor: Some("Eclipse Adoptium".into()),
    };
    validate_detected(&home, &package, &detected).unwrap();
}

#[cfg(unix)]
#[async_std::test]
async fn validation_registers_actual_jvm_version_when_catalog_version_is_abbreviated() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let db = Arc::new(
        crate::data::db::Database::init("sqlite::memory:")
            .await
            .unwrap(),
    );
    let java = JavaService::new(db, crate::bus::EventBus::new(16), dir.path().into());
    java.managed.initialize().await.unwrap();
    let mut package = package();
    package.major = 27;
    package.version = "27.0.1.0".into();
    let mut rec = record(package, ManagedJavaStage::AwaitingValidation);
    rec.home = Some(PathBuf::from("managed/test/jre"));
    let home = dir.path().join(rec.home.as_ref().unwrap());
    std::fs::create_dir_all(home.join("bin")).unwrap();
    let exec = extract::executable(&home);
    let script = format!(
        "#!/bin/sh\ncat >&2 <<'JAVA'\njava.home = {}\njava.runtime.version = 27.0.1.0.2+1\njava.specification.version = 27\nos.arch = {}\njava.vendor = Eclipse Adoptium\nJAVA\n",
        home.display(), crate::java::select::host_arch()
    );
    std::fs::write(&exec, script).unwrap();
    std::fs::set_permissions(&exec, std::fs::Permissions::from_mode(0o755)).unwrap();
    let id = insert(&java.managed, rec);
    let runtime = java.managed.validate(id, &java).await.unwrap();
    assert_eq!(runtime.version, "27.0.1.0.2+1");
    assert_eq!(runtime.major, 27);
    assert_eq!(runtime.source, JavaSource::Managed);
    assert_eq!(java.list_all().await.unwrap()[0].version, "27.0.1.0.2+1");
    let snapshot = java.managed.installation(id).await.unwrap();
    assert_eq!(snapshot.stage, ManagedJavaStage::Ready);
    assert_eq!(snapshot.runtime_id, Some(runtime.id));
    assert!(snapshot.error.is_none());
}

#[async_std::test]
async fn returned_failure_matches_snapshot_when_saving_diagnostics_fails() {
    let dir = tempfile::tempdir().unwrap();
    let service = ManagedJavaService::new(dir.path().into());
    service.initialize().await.unwrap();
    let id = insert(&service, record(package(), ManagedJavaStage::Validating));
    std::fs::create_dir_all(
        dir.path()
            .join(".managed/operations")
            .join(format!("{id}.json")),
    )
    .unwrap();
    let error = service
        .fail(id, ManagedJavaError::new("validation", "Probe failed"))
        .await;
    assert_eq!(error.stage, Some(ManagedJavaStage::Validating));
    assert!(error.details.contains("Saving failure details failed"));
    assert_eq!(service.installation(id).await.unwrap().error, Some(error));
}
