use async_std::fs;
use httpmock::{Method::GET, MockServer};
use tempfile::tempdir;

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

#[async_std::test]
async fn checksum_success() {
    let server = MockServer::start_async().await;

    let body = b"Hello, test payload";
    let mut hasher = Sha1::new();
    hasher.update(body);
    let expected_hash = format!("{:x}", hasher.finalize());

    server
        .mock_async(|when, then| {
            when.method(GET).path("/file.bin");
            then.status(200).body(body);
        })
        .await;

    let tmp = tempdir().unwrap();
    let dir_path = tmp.path().to_path_buf();
    let file_name = PathBuf::from("file.bin");
    let object = TestObj {
        name: "test".into(),
        url: server.url("/file.bin"),
        hash: expected_hash,
    };
    let pool = Arc::new(BufferPool::new(1, 4096));
    let dl = Download::new(dir_path.join(file_name.clone()), object.clone(), pool);

    // Execute the function
    let res = dl.download_with_checksum().await;
    assert!(res.is_ok(), "Expected Ok(_), got Err: {:?}", res.err());

    // Make sure the file is written
    let ret = res.unwrap();
    assert_eq!(ret.get_name(), &object.name);

    let saved_path = dir_path.join(file_name);
    let saved = fs::read(saved_path).await.unwrap();
    assert_eq!(saved.as_slice(), body);
}

#[async_std::test]
async fn checksum_mismatch() {
    let server = MockServer::start_async().await;

    let body = b"Content that won't match";
    server
        .mock_async(|when, then| {
            when.method(GET).path("/file.bin");
            then.status(200).body(body);
        })
        .await;

    let tmp = tempdir().unwrap();
    let dir_path = tmp.path().to_path_buf();
    let file_name = PathBuf::from("file.bin");
    let mismatch_hash = "0000000000000000000000000000000000000000";
    let object = TestObj {
        name: "test".into(),
        url: server.url("/file.bin"),
        hash: mismatch_hash.into(),
    };
    let pool = Arc::new(BufferPool::new(1, DEFAULT_BUFFER_SIZE));
    let dl = Download::new(dir_path.join(file_name.clone()), object.clone(), pool);

    let res = dl.download_with_checksum().await;
    println!("{:#?}", res);
    assert!(res.is_err());
    assert!(res.err().unwrap().to_lowercase().contains("sha1"));
}

#[async_std::test]
async fn too_many_redirects() {
    let server = MockServer::start_async().await;
    server
        .mock_async(|when, then| {
            when.method(GET).path("/loop");
            then.status(302).header("Location", "/loop");
        })
        .await;

    let tmp = tempdir().unwrap();
    let dir_path = tmp.path().to_path_buf();
    let file_name = PathBuf::from("file.bin");
    let object = TestObj {
        name: "test".into(),
        url: server.url("/loop"),
        hash: "".into(),
    };
    let pool = Arc::new(BufferPool::new(1, DEFAULT_BUFFER_SIZE));
    let dl = Download::new(dir_path.join(file_name), object, pool);

    let res = dl.download_with_checksum().await;
    assert!(res
        .err()
        .unwrap()
        .to_lowercase()
        .contains("too many redirects when fetcing"));
}

#[async_std::test]
async fn redirect_success() {
    let server = MockServer::start_async().await;

    let body = b"File reached";
    let mut hasher = Sha1::new();
    hasher.update(body);
    let expected_hash = format!("{:x}", hasher.finalize());

    server
        .mock_async(|when, then| {
            when.method(GET).path("/file.bin");
            then.status(302).header("Location", "/file_new.bin");
        })
        .await;

    server
        .mock_async(|when, then| {
            when.method(GET).path("/file_new.bin");
            then.status(200).body(body);
        })
        .await;

    let tmp = tempdir().unwrap();
    let dir_path = tmp.path().to_path_buf();
    let file_name = PathBuf::from("file.bin");
    let object = TestObj {
        name: "test".into(),
        url: server.url("/file.bin"),
        hash: expected_hash
    };
    let pool = Arc::new(BufferPool::new(1, DEFAULT_BUFFER_SIZE));
    let dl = Download::new(dir_path.join(file_name), object, pool);

    let res = dl.download_with_checksum().await;
    println!("{:#?}", res);
    assert!(res.is_ok());
}

#[async_std::test]
async fn redirect_no_location() {
    let server = MockServer::start_async().await;

    let body = b"";
    let mut hasher = Sha1::new();
    hasher.update(body);
    let expected_hash = format!("{:x}", hasher.finalize());

    server
        .mock_async(|when, then| {
            when.method(GET).path("/file.bin");
            then.status(302);
        })
        .await;

    let tmp = tempdir().unwrap();
    let dir_path = tmp.path().to_path_buf();
    let file_name = PathBuf::from("file.bin");
    let object = TestObj {
        name: "test".into(),
        url: server.url("/file.bin"),
        hash: expected_hash
    };
    let pool = Arc::new(BufferPool::new(1, DEFAULT_BUFFER_SIZE));
    let dl = Download::new(dir_path.join(file_name), object, pool);

    let res = dl.download_with_checksum().await;
    println!("{:#?}", res);
    assert!(res.err().unwrap().to_lowercase().contains("without location"));
}

#[async_std::test]
async fn read_zero_bytes() {
    let server = MockServer::start_async().await;

    let body = b"";
    let mut hasher = Sha1::new();
    hasher.update(body);
    let expected_hash = format!("{:x}", hasher.finalize());

    server
        .mock_async(|when, then| {
            when.method(GET).path("/file.bin");
            then.status(200).body(body);
        })
        .await;

    let tmp = tempdir().unwrap();
    let dir_path = tmp.path().to_path_buf();
    let file_name = PathBuf::from("file.bin");
    let object = TestObj {
        name: "test".into(),
        url: server.url("/file.bin"),
        hash: expected_hash
    };
    let pool = Arc::new(BufferPool::new(1, DEFAULT_BUFFER_SIZE));
    let dl = Download::new(dir_path.join(file_name), object, pool);

    let res = dl.download_with_checksum().await;
    println!("{:#?}", res);
    assert!(res.err().unwrap().to_lowercase().contains("read 0 bytes"));
}
