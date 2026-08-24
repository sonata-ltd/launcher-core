use std::time::Duration;

use crate::utils::download::sweep::{is_partial, sweep_partials};
use async_std::{fs, stream::StreamExt};
use httpmock::{Method::GET, MockServer};
use tempfile::{tempdir, TempDir};

use super::*;

const DEFAULT_BUFFER_SIZE: usize = 4096;

#[derive(Debug, Clone, PartialEq)]
struct TestObj {
    name: String,
    hash: String,
    url: String,
}

impl Downloadable for TestObj {
    fn get_name(&self) -> &String {
        &self.name
    }

    fn get_hash(&self) -> &String {
        &self.hash
    }

    fn get_url(&self) -> &String {
        &self.url
    }
}

fn sha1_hex(body: &[u8]) -> String {
    let mut hasher = Sha1::new();
    hasher.update(body);
    format!("{:x}", hasher.finalize())
}

struct Fixture {
    _tmp: TempDir,
    dir: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let tmp = tempdir().unwrap();
        let dir = tmp.path().to_path_buf();

        Self { _tmp: tmp, dir }
    }

    fn download(&self, url: String, hash: String) -> Download<TestObj> {
        Download::new(
            self.dir.join("file.bin"),
            TestObj {
                name: "test".into(),
                url,
                hash,
            },
            Arc::new(BufferPool::new(1, DEFAULT_BUFFER_SIZE)),
        )
    }
}

async fn entries(dir: &Path) -> Vec<String> {
    let mut names = Vec::new();
    let mut rd = fs::read_dir(dir).await.unwrap();

    while let Some(entry) = rd.next().await {
        names.push(entry.unwrap().file_name().to_string_lossy().into_owned());
    }

    names.sort();
    names
}

#[async_std::test]
async fn empty_body_with_matching_hash_succeeds() {
    let server = MockServer::start_async().await;
    let body: &[u8] = b"";

    server
        .mock_async(|when, then| {
            when.method(GET).path("/file.bin");
            then.status(200).body(body);
        })
        .await;

    let fx = Fixture::new();
    let res = fx
        .download(server.url("/file.bin"), sha1_hex(body))
        .download_with_checksum()
        .await;

    assert!(
        res.is_ok(),
        "empty file with a matching hash is valid: {:?}",
        res.err()
    );
    assert_eq!(entries(&fx.dir).await, vec!["file.bin"]);
    assert!(fs::read(fx.dir.join("file.bin")).await.unwrap().is_empty());
}

#[async_std::test]
async fn failed_checksum_leaves_nothing_behind() {
    let server = MockServer::start_async().await;

    server
        .mock_async(|when, then| {
            when.method(GET).path("/file.bin");
            then.status(200).body(b"payload that will not match");
        })
        .await;

    let fx = Fixture::new();
    let res = fx
        .download(server.url("/file.bin"), "0".repeat(40))
        .download_with_checksum()
        .await;

    assert!(matches!(res, Err(DownloadError::ChecksumMismatch { .. })));

    assert!(entries(&fx.dir).await.is_empty());
}

#[async_std::test]
async fn http_error_leaves_nothing_behind() {
    let server = MockServer::start_async().await;

    server
        .mock_async(|when, then| {
            when.method(GET).path("/file.bin");
            then.status(503);
        })
        .await;

    let fx = Fixture::new();
    let res = fx
        .download(server.url("/file.bin"), sha1_hex(b""))
        .download_with_checksum()
        .await;

    match res {
        Err(e @ DownloadError::Http { .. }) => {
            assert!(e.is_transient(), "503 must be retryable");
        }
        other => panic!("expected Http, got {other:?}"),
    }

    assert!(entries(&fx.dir).await.is_empty());
}

#[async_std::test]
async fn overwrites_a_corrupted_existing_file() {
    let server = MockServer::start_async().await;
    let body = b"fresh content";

    server
        .mock_async(|when, then| {
            when.method(GET).path("/file.bin");
            then.status(200).body(body);
        })
        .await;

    let fx = Fixture::new();

    fs::write(fx.dir.join("file.bin"), b"stale garbage, noticeably longer")
        .await
        .unwrap();

    let res = fx
        .download(server.url("/file.bin"), sha1_hex(body))
        .download_with_checksum()
        .await;

    assert!(res.is_ok());
    assert_eq!(fs::read(fx.dir.join("file.bin")).await.unwrap(), body);
    assert_eq!(entries(&fx.dir).await, vec!["file.bin"]);
}

#[async_std::test]
async fn invalid_url_is_an_error_not_a_panic() {
    let fx = Fixture::new();
    let res = fx
        .download("not a url".to_string(), sha1_hex(b""))
        .download_with_checksum()
        .await;

    assert!(matches!(res, Err(DownloadError::InvalidUrl { .. })));
}

const UUID: &str = "0123456789abcdef0123456789abcdef";

#[async_std::test]
async fn sweep_removes_partials_recursively() {
    let tmp = tempdir().unwrap();
    let root = tmp.path();
    let deep = root.join("com/mojang/logging/0.1.5");

    fs::create_dir_all(&deep).await.unwrap();
    fs::write(deep.join(format!(".logging.jar.{UUID}.part")), b"x")
        .await
        .unwrap();
    fs::write(deep.join("logging.jar"), b"x").await.unwrap();
    fs::write(root.join(format!(".top.jar.{UUID}.part")), b"x")
        .await
        .unwrap();

    let removed = sweep_partials(root, Duration::ZERO).await;

    assert_eq!(removed, 2);
    assert_eq!(entries(&deep).await, vec!["logging.jar"]);
    assert_eq!(entries(root).await, vec!["com"]);
}

#[async_std::test]
async fn sweep_keeps_fresh_partials() {
    let tmp = tempdir().unwrap();
    let root = tmp.path();
    let name = format!(".lib.jar.{UUID}.part");

    fs::write(root.join(&name), b"x").await.unwrap();

    assert_eq!(sweep_partials(root, Duration::from_secs(3600)).await, 0);
    assert_eq!(entries(root).await, vec![name]);
}

#[async_std::test]
async fn sweep_spares_foreign_part_files() {
    let tmp = tempdir().unwrap();
    let root = tmp.path();

    fs::write(root.join("movie.mkv.part"), b"x").await.unwrap();
    fs::write(root.join("notes.part"), b"x").await.unwrap();

    assert_eq!(sweep_partials(root, Duration::ZERO).await, 0);
    assert_eq!(entries(root).await, vec!["movie.mkv.part", "notes.part"]);
}

#[async_std::test]
async fn sweep_tolerates_missing_root() {
    let tmp = tempdir().unwrap();

    assert_eq!(
        sweep_partials(&tmp.path().join("never-created"), Duration::ZERO).await,
        0
    );
}

#[test]
fn is_partial_matches_only_our_naming() {
    for name in [
        format!(".lib.jar.{UUID}.part"),
        format!(".{}.{UUID}.part", "a".repeat(40)),
    ] {
        assert!(is_partial(Path::new(&name)), "should match: {name}");
    }

    for name in [
        "lib.jar".to_string(),
        "movie.mkv.part".to_string(),
        format!("lib.jar.{UUID}"),
        format!("lib.jar.{UUID}.part"),
        ".lib.jar.not-a-uuid.part".to_string(),
        format!(".lib.jar.{}.part", &UUID[..31]),
    ] {
        assert!(!is_partial(Path::new(&name)), "should not match: {name}");
    }
}

#[test]
fn generated_temp_names_are_recognized_as_partial() {
    let path = temp_path(Path::new("/tmp/libs"), OsStr::new("logging-0.1.5.jar"));

    assert!(is_partial(&path));
    assert_eq!(path.parent(), Some(Path::new("/tmp/libs")));

    let other = temp_path(Path::new("/tmp/libs"), OsStr::new("logging-0.1.5.jar"));
    assert_ne!(path, other);
}
