//! On-disk identity shared by repositories of downloaded metadata.
use crate::utils::{files, object_store::catalog::read_regular};
use std::path::{Path, PathBuf};

pub(crate) fn entry_path(directory: &Path, url: &str) -> PathBuf {
    directory.join(format!("{}.json", files::sha256(url.as_bytes())))
}

pub(crate) async fn publish(path: PathBuf, bytes: Vec<u8>) -> Result<(), files::FileError> {
    async_std::task::spawn_blocking(move || files::atomic_write(&path, &bytes)).await
}

pub(crate) async fn read(path: &Path) -> std::io::Result<Vec<u8>> {
    let path = path.to_owned();
    async_std::task::spawn_blocking(move || read_regular(&path, super::MAX_MANIFEST_BYTES as u64))
        .await
}
