use super::*;
use httpmock::{Method::GET, MockServer};

#[async_std::test]
async fn cancelled_download_preserves_existing_destination_without_network() {
    let temp = tempfile::tempdir().unwrap();
    let destination = temp.path().join("client.jar");
    fs::write(&destination, b"previous runtime").await.unwrap();
    let cancel = Cancel::default();
    cancel.cancel();

    let error = download("not-a-network-url", &destination, None, &cancel)
        .await
        .unwrap_err();

    assert!(error.is_cancelled());
    assert_eq!(fs::read(destination).await.unwrap(), b"previous runtime");
    assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 1);
}

#[async_std::test]
async fn malformed_hash_is_rejected_before_request_or_file_creation() {
    let server = MockServer::start();
    let mock = server.mock(|when, then| {
        when.method(GET).path("/artifact");
        then.status(200).body("data");
    });
    let temp = tempfile::tempdir().unwrap();

    let error = download(
        &server.url("/artifact"),
        &temp.path().join("artifact.jar"),
        Some("not-a-sha1"),
        &Cancel::default(),
    )
    .await
    .unwrap_err();

    assert!(matches!(error, DownloadError::Invalid(_)));
    mock.assert_hits(0);
    assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 0);
}

#[async_std::test]
async fn checksum_mismatch_is_not_retried_and_keeps_previous_file() {
    let server = MockServer::start();
    let mock = server.mock(|when, then| {
        when.method(GET).path("/artifact");
        then.status(200).body("corrupt content");
    });
    let temp = tempfile::tempdir().unwrap();
    let destination = temp.path().join("artifact.jar");
    fs::write(&destination, b"previous runtime").await.unwrap();
    let expected = "0".repeat(40);
    let url = server.url("/artifact");

    let error = download(&url, &destination, Some(&expected), &Cancel::default())
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        DownloadError::ChecksumMismatch { url: actual_url, expected: actual_expected, actual }
            if actual_url == url && actual_expected == expected
                && actual == format!("{:x}", Sha1::digest(b"corrupt content"))
    ));
    mock.assert_hits(1);
    assert_eq!(fs::read(destination).await.unwrap(), b"previous runtime");
    assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 1);
}

#[async_std::test]
async fn http_failure_keeps_three_attempt_limit() {
    let server = MockServer::start();
    let mock = server.mock(|when, then| {
        when.method(GET).path("/artifact");
        then.status(503);
    });
    let temp = tempfile::tempdir().unwrap();

    let error = download(
        &server.url("/artifact"),
        &temp.path().join("artifact.jar"),
        None,
        &Cancel::default(),
    )
    .await
    .unwrap_err();

    assert!(matches!(error, DownloadError::Http(_)));
    mock.assert_hits(3);
    assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 0);
}
