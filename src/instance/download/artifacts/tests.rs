use super::*;
use crate::{
    bus::EventBus,
    data::registry::operation::{message::status::Outcome, OperationRegistry},
    instance::{download::error::DownloadError, rules::RuleError},
    utils::files::FileError,
};

#[test]
fn malformed_metadata_and_maven_coordinates_have_distinct_errors() {
    let error = parse(&mut json!({"libraries": [{}]}), &[]).err().unwrap();
    assert!(matches!(error, ArtifactError::Invalid(_)));

    let error = parse(&mut json!({"libraries": [{"name": "malformed"}]}), &[])
        .err()
        .unwrap();
    assert!(matches!(error, ArtifactError::Maven(_)));
}

#[test]
fn unsafe_artifact_path_preserves_file_error() {
    let mut manifest = json!({"libraries": [{
        "name": "g:a:1",
        "downloads": {"artifact": {"path": "../escape.jar"}}
    }]});

    let error = parse(&mut manifest, &[]).err().unwrap();

    assert!(matches!(
        error,
        ArtifactError::File(FileError::InvalidPath(_))
    ));
}

#[test]
fn rejects_conflicting_hashes_for_same_artifact() {
    let mut manifest = json!({"libraries": [
        {"name": "g:a:1", "downloads": {"artifact": {"sha1": "0".repeat(40)}}},
        {"name": "g:a:1", "downloads": {"artifact": {"sha1": "1".repeat(40)}}}
    ]});

    let error = parse(&mut manifest, &[]).err().unwrap();

    assert!(matches!(error, ArtifactError::Invalid(message)
        if message == "different hashes for same artifact path"));
}

#[test]
fn library_rule_errors_keep_their_source_types() {
    let mut manifest = json!({"libraries": [{
        "name": "g:a:1",
        "rules": [{"action": "allow", "os": {"version": "["}}]
    }]});

    let error = parse(&mut manifest, &[]).err().unwrap();

    assert!(matches!(error, ArtifactError::Rules(RuleError::Regex(_))));
}

#[async_std::test]
async fn hydration_cancellation_survives_artifact_wrapper() {
    let temp = tempfile::tempdir().unwrap();
    let operations = OperationRegistry::new(EventBus::new(16));
    let op = operations.begin("test".into(), vec![OperationStage::DownloadLibs]);
    let cancel = Cancel::default();
    cancel.cancel();
    let wanted = [Wanted {
        path: "client.jar".into(),
        url: Some("not-a-network-url".into()),
        sha1: None,
        classpath: true,
        native: false,
    }];

    let error = hydrate(
        &wanted,
        temp.path(),
        &temp.path().join("blobs"),
        &cancel,
        &op,
    )
    .await
    .unwrap_err();

    assert!(error.is_cancelled());
    assert!(matches!(
        error,
        ArtifactError::Download(DownloadError::Cancelled)
    ));
    assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 0);
    op.finish(Outcome::Cancelled);
}

#[async_std::test]
async fn verification_returns_checksum_details_without_changing_file() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("client.jar");
    std::fs::write(&path, b"corrupt content").unwrap();
    let actual = files::sha1_file(&path).unwrap();
    let expected = "0".repeat(40);
    let wanted = vec![Wanted {
        path: "client.jar".into(),
        url: None,
        sha1: Some(expected.clone()),
        classpath: true,
        native: false,
    }];
    let operations = OperationRegistry::new(EventBus::new(16));
    let op = operations.begin("test".into(), vec![OperationStage::VerifyFiles]);

    let error = finish(wanted, temp.path().to_path_buf(), &op)
        .await
        .unwrap_err();

    assert!(!error.is_cancelled());
    assert!(matches!(error, ArtifactError::ChecksumMismatch {
        path: failed_path, expected: failed_expected, actual: failed_actual,
    } if failed_path == path && failed_expected == expected && failed_actual == actual));
    assert_eq!(std::fs::read(path).unwrap(), b"corrupt content");
    op.finish(Outcome::Failed);
}

#[async_std::test]
async fn missing_generated_file_remains_an_io_error() {
    let temp = tempfile::tempdir().unwrap();
    let operations = OperationRegistry::new(EventBus::new(16));
    let op = operations.begin("test".into(), vec![OperationStage::VerifyFiles]);

    let error = finish(
        vec![Wanted {
            path: "missing.jar".into(),
            url: None,
            sha1: None,
            classpath: true,
            native: false,
        }],
        temp.path().to_path_buf(),
        &op,
    )
    .await
    .unwrap_err();

    assert!(matches!(error, ArtifactError::Io(source)
        if source.kind() == std::io::ErrorKind::NotFound));
    op.finish(Outcome::Failed);
}
