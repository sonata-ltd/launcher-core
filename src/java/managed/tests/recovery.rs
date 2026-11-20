use super::*;
use std::fs;

#[async_std::test]
async fn corrupt_oversized_future_or_unsafe_record_does_not_hide_healthy_installations_or_change_bad_bytes(
) {
    for mode in [
        "json",
        "oversized",
        "future",
        "unsafe",
        "identity",
        "unknown-file",
    ] {
        let dir = tempfile::tempdir().unwrap();
        let service = ManagedJavaService::new(dir.path().into());
        service.initialize().await.unwrap();
        let healthy = record(package(), ManagedJavaStage::Ready);
        let healthy_id = insert(&service, healthy);
        service.persist(healthy_id).await.unwrap();
        let bad = record(package(), ManagedJavaStage::Downloading);
        let bad_id = bad.snapshot.id;
        let path = dir
            .path()
            .join(".managed/operations")
            .join(if mode == "unknown-file" {
                "unfinished-record".into()
            } else {
                format!("{bad_id}.json")
            });
        let bytes = match mode {
            "json" | "unknown-file" => b"broken JSON".to_vec(),
            "oversized" => vec![b' '; 1024 * 1024 + 1],
            _ => {
                let mut json = serde_json::to_value(&bad).unwrap();
                match mode {
                    "future" => json["snapshot"]["future_schema"] = true.into(),
                    "unsafe" => json["home"] = "../outside".into(),
                    _ => json["snapshot"]["id"] = Uuid::new_v4().to_string().into(),
                }
                serde_json::to_vec(&json).unwrap()
            }
        };
        fs::write(&path, &bytes).unwrap();
        let loaded = ManagedJavaService::new(dir.path().into());
        let report = loaded.installations_report().await.unwrap();
        assert_eq!(report.installations.len(), 1, "{mode}");
        assert_eq!(report.installations[0].id, healthy_id);
        assert_eq!(report.diagnostics.len(), 1, "{mode}");
        assert_eq!(report.diagnostics[0].path, path);
        assert_eq!(loaded.installations().await.unwrap().len(), 1);
        assert_eq!(
            loaded.installation(healthy_id).await.unwrap().stage,
            ManagedJavaStage::Ready
        );
        if mode != "unknown-file" {
            assert_eq!(
                loaded.installation(bad_id).await.unwrap_err().code,
                "recovery_record"
            );
        }
        assert_eq!(fs::read(path).unwrap(), bytes);
    }
}

#[async_std::test]
async fn cleanup_failure_is_diagnosed_per_record_and_other_dead_owned_operation_recovers() {
    let dir = tempfile::tempdir().unwrap();
    let service = ManagedJavaService::new(dir.path().into());
    service.initialize().await.unwrap();
    let mut bad = record(package(), ManagedJavaStage::Downloading);
    bad.owner = Some(dead_owner(bad.snapshot.id));
    let bad_id = insert(&service, bad);
    create_staging(&service, bad_id);
    service.persist(bad_id).await.unwrap();
    fs::write(
        service.staging(bad_id).join(".sonata-owner.json"),
        b"broken marker",
    )
    .unwrap();
    fs::write(service.staging(bad_id).join("keep"), b"unknown bytes").unwrap();
    let mut good = record(package(), ManagedJavaStage::Downloading);
    good.owner = Some(dead_owner(good.snapshot.id));
    let good_id = insert(&service, good);
    create_staging(&service, good_id);
    service.persist(good_id).await.unwrap();
    let bad_path = dir
        .path()
        .join(".managed/operations")
        .join(format!("{bad_id}.json"));
    let original = fs::read(&bad_path).unwrap();
    let loaded = ManagedJavaService::new(dir.path().into());
    let report = loaded.installations_report().await.unwrap();
    assert_eq!(report.installations.len(), 2);
    assert_eq!(report.diagnostics.len(), 1);
    assert_eq!(report.diagnostics[0].operation, Some(bad_id));
    assert_eq!(
        loaded.installation(bad_id).await.unwrap().stage,
        ManagedJavaStage::Downloading
    );
    assert_eq!(
        loaded.installation(good_id).await.unwrap().stage,
        ManagedJavaStage::Failed
    );
    assert!(!service.staging(good_id).exists());
    assert_eq!(
        fs::read(service.staging(bad_id).join("keep")).unwrap(),
        b"unknown bytes"
    );
    assert_eq!(fs::read(bad_path).unwrap(), original);
}

#[cfg(unix)]
#[async_std::test]
async fn linked_record_is_reported_without_reading_or_modifying_external_target() {
    let dir = tempfile::tempdir().unwrap();
    let service = ManagedJavaService::new(dir.path().into());
    service.initialize().await.unwrap();
    let healthy_id = insert(&service, record(package(), ManagedJavaStage::Ready));
    service.persist(healthy_id).await.unwrap();
    let outside = dir.path().join("external");
    fs::write(&outside, b"external secret bytes").unwrap();
    let id = Uuid::new_v4();
    let linked = dir
        .path()
        .join(".managed/operations")
        .join(format!("{id}.json"));
    std::os::unix::fs::symlink(&outside, &linked).unwrap();
    let loaded = ManagedJavaService::new(dir.path().into());
    let report = loaded.installations_report().await.unwrap();
    assert_eq!(report.installations.len(), 1);
    assert_eq!(report.diagnostics[0].path, linked);
    assert!(!report.diagnostics[0]
        .error
        .details
        .contains("external secret bytes"));
    assert_eq!(fs::read(outside).unwrap(), b"external secret bytes");
}

#[async_std::test]
async fn query_isolation_never_turns_corrupt_record_into_permission_for_gc() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("java");
    let db = Arc::new(
        crate::data::db::Database::init(&format!(
            "sqlite://{}",
            dir.path().join("owner.db").display()
        ))
        .await
        .unwrap(),
    );
    let service = ManagedJavaService::at(root.clone(), db);
    service.initialize().await.unwrap();
    let healthy_id = insert(&service, record(package(), ManagedJavaStage::Ready));
    service.persist(healthy_id).await.unwrap();
    let corrupt = root
        .join(".managed/operations")
        .join(format!("{}.json", Uuid::new_v4()));
    fs::write(&corrupt, b"broken root record").unwrap();
    let loaded = ManagedJavaService::new(root.clone());
    let report = loaded.installations_report().await.unwrap();
    assert_eq!(report.installations.len(), 1);
    assert_eq!(report.diagnostics.len(), 1);
    assert!(gc::collect(root, std::time::Duration::ZERO).await.is_err());
    assert_eq!(fs::read(corrupt).unwrap(), b"broken root record");
}

#[cfg(unix)]
#[async_std::test]
async fn recovery_declines_a_redirected_operations_directory_without_touching_its_target() {
    let dir = tempfile::tempdir().unwrap();
    let elsewhere = tempfile::tempdir().unwrap();
    let kept = elsewhere.path().join("kept");
    fs::write(&kept, b"keep").unwrap();
    fs::create_dir_all(dir.path().join(".managed")).unwrap();
    std::os::unix::fs::symlink(elsewhere.path(), dir.path().join(".managed/operations")).unwrap();

    let service = ManagedJavaService::new(dir.path().into());
    assert!(service.initialize().await.is_err());
    assert_eq!(fs::read(&kept).unwrap(), b"keep");
    assert_eq!(fs::read_dir(elsewhere.path()).unwrap().count(), 1);
}
