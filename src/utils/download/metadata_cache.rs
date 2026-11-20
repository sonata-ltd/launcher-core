//! On-disk identity shared by repositories of downloaded metadata.
use crate::utils::{
    files::{self, directory::Directory},
    object_store::catalog::read_open_regular,
};
use std::{
    ffi::OsString,
    io,
    path::{Path, PathBuf},
    sync::Arc,
    time::SystemTime,
};

pub(crate) fn entry_path(directory: &Path, url: &str) -> PathBuf {
    directory.join(format!("{}.json", files::sha256(url.as_bytes())))
}

#[cfg(test)]
pub(crate) async fn publish(path: PathBuf, bytes: Vec<u8>) -> Result<(), files::FileError> {
    async_std::task::spawn_blocking(move || files::atomic_write(&path, &bytes)).await
}

pub(crate) struct Entry {
    directory: Arc<Directory>,
    name: OsString,
}

impl Entry {
    pub(crate) fn new(directory: Arc<Directory>, url: &str) -> Self {
        let name = format!("{}.json", files::sha256(url.as_bytes())).into();
        Self { directory, name }
    }

    pub(crate) async fn read(&self) -> io::Result<(Vec<u8>, Option<SystemTime>)> {
        let directory = self.directory.clone();
        let name = self.name.clone();
        async_std::task::spawn_blocking(move || {
            let file = directory.open_file(&name)?;
            let modified = file
                .metadata()
                .ok()
                .and_then(|metadata| metadata.modified().ok());
            let bytes = read_open_regular(&file, super::MAX_MANIFEST_BYTES as u64)?;
            Ok((bytes, modified))
        })
        .await
    }

    pub(crate) async fn publish(self, bytes: Vec<u8>) -> io::Result<()> {
        async_std::task::spawn_blocking(move || self.directory.write_bytes(&self.name, &bytes))
            .await
    }
}
