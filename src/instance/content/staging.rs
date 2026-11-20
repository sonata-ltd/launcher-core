//! Private download/publication scratch. Recovery uses CAS, never these copies.
use std::{
    ffi::OsStr,
    fs, io,
    path::{Path, PathBuf},
    sync::Arc,
};

use crate::utils::files::{
    directory::Directory,
    owned_directory::{self, OwnedDirectory},
};

const PREFIX: &str = ".sonata-content-";

pub(super) struct ContentStaging {
    files: Arc<Directory>,
    downloads: Arc<Directory>,
    _directory: OwnedDirectory,
}

impl ContentStaging {
    #[cfg(test)]
    pub(super) fn create(root: &Path) -> io::Result<Self> {
        fs::create_dir_all(root)?;
        Self::create_in(&Directory::open(root)?)
    }

    pub(super) fn create_in(root: &Arc<Directory>) -> io::Result<Self> {
        let directory = OwnedDirectory::create_in(root, PREFIX)?;
        // Keep provider-controlled filenames away from the ownership marker
        // and the download namespace, even if a provider chooses a hidden name.
        let files = directory
            .directory()
            .create_private_child(OsStr::new("files"))?;
        let downloads = directory
            .directory()
            .create_private_child(OsStr::new("downloads"))?;
        Ok(Self {
            files,
            downloads,
            _directory: directory,
        })
    }

    #[cfg(test)]
    pub(super) fn files(&self) -> &Path {
        self.files.path()
    }

    pub(super) fn files_directory(&self) -> &Arc<Directory> {
        &self.files
    }

    pub(super) fn downloads_directory(&self) -> &Arc<Directory> {
        &self.downloads
    }
}

pub(super) async fn sweep_stale(instances_root: PathBuf) {
    async_std::task::spawn_blocking(move || match sweep(&instances_root) {
        Ok(removed) if removed > 0 => {
            tracing::info!(removed, "cleaned up abandoned content staging")
        }
        Ok(_) => {}
        Err(error) => tracing::warn!(%error, "could not sweep content staging"),
    })
    .await;
}

fn sweep(instances_root: &Path) -> io::Result<usize> {
    match fs::symlink_metadata(instances_root) {
        Ok(metadata) if metadata.is_dir() => {}
        Ok(_) => return Ok(0),
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(error),
    }
    let mut removed = 0;
    for instance in fs::read_dir(instances_root)? {
        let instance = instance?;
        if instance.file_type()?.is_dir() {
            removed += owned_directory::sweep(&instance.path(), PREFIX)?;
        }
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_scratch_is_private_and_keeps_live_and_legacy_directories() {
        let temp = tempfile::tempdir().unwrap();
        let instance = temp.path().join("instance");
        let stage = ContentStaging::create(&instance).unwrap();
        let legacy = instance.join(format!("{PREFIX}legacy"));
        fs::create_dir(&legacy).unwrap();
        fs::write(legacy.join("old.jar"), b"untracked staged bytes").unwrap();
        fs::write(stage.files().join(".sonata-owner"), b"provider filename").unwrap();
        fs::write(
            stage.downloads_directory().path().join("0.jar"),
            b"downloaded bytes",
        )
        .unwrap();
        assert_eq!(
            crate::utils::process::read_owner(&stage._directory.path().join(".sonata-owner")),
            Some(std::process::id())
        );
        assert_eq!(sweep(temp.path()).unwrap(), 0);
        let directory = stage._directory.path().to_owned();
        drop(stage);
        assert!(!directory.exists());
        assert_eq!(
            fs::read(legacy.join("old.jar")).unwrap(),
            b"untracked staged bytes"
        );
        assert_eq!(sweep(&temp.path().join("absent")).unwrap(), 0);
    }

    #[cfg(unix)]
    #[test]
    fn content_staging_sweep_does_not_enter_symlinked_instances() {
        let temp = tempfile::tempdir().unwrap();
        let instances = temp.path().join("instances");
        let outside = temp.path().join("outside");
        fs::create_dir(&instances).unwrap();
        fs::create_dir(&outside).unwrap();
        let stage = outside.join(format!("{PREFIX}dead"));
        fs::create_dir(&stage).unwrap();
        fs::write(stage.join(".sonata-owner"), b"2147483647").unwrap();
        fs::write(stage.join("data"), b"external bytes").unwrap();
        std::os::unix::fs::symlink(&outside, instances.join("linked-instance")).unwrap();
        assert_eq!(sweep(&instances).unwrap(), 0);
        assert_eq!(fs::read(stage.join("data")).unwrap(), b"external bytes");
    }
}
