use std::{
    ffi::OsString,
    io,
    path::{Path, PathBuf},
    sync::Arc,
};

use crate::utils::{
    cancellation::Cancellation,
    files::{self, directory::Directory},
};

use super::sweep::{OwnedEntry, PartialOwner};

pub(crate) enum Destination {
    Path(PathBuf),
    Within(Arc<Directory>, PathBuf),
}

impl From<PathBuf> for Destination {
    fn from(path: PathBuf) -> Self {
        Self::Path(path)
    }
}

impl Destination {
    pub(super) fn path(&self) -> PathBuf {
        match self {
            Self::Path(path) => path.clone(),
            Self::Within(root, relative) => root.path().join(relative),
        }
    }

    pub(super) fn bind(self) -> io::Result<Target> {
        let entry = match &self {
            Self::Path(path) | Self::Within(_, path) => path,
        };
        let name = entry
            .file_name()
            .ok_or(io::ErrorKind::InvalidInput)?
            .to_owned();
        let directory = match self {
            Self::Path(path) => {
                Directory::ensure_path(path.parent().ok_or(io::ErrorKind::InvalidInput)?)?
            }
            Self::Within(root, path) => root.create_entry_parent(&path)?,
        };
        Ok(Target { directory, name })
    }
}

pub(super) struct Target {
    pub(super) directory: Arc<Directory>,
    pub(super) name: OsString,
}

impl Target {
    pub(super) fn path(&self) -> PathBuf {
        self.directory.path().join(&self.name)
    }

    pub(super) fn matches(&self, hash: &str, cancel: &Cancellation) -> io::Result<bool> {
        self.directory.verify_location()?;
        let matched = files::matches_open_file(self.directory.open_file(&self.name), hash, cancel)?;
        self.directory.verify_location()?;
        Ok(matched)
    }

    pub(super) fn stage(&self) -> io::Result<(Partial, async_std::fs::File)> {
        let path = super::temp_path(self.directory.path(), Path::new("download").as_os_str());
        let name = path
            .file_name()
            .ok_or(io::ErrorKind::InvalidInput)?
            .to_owned();
        let owner = PartialOwner::create_in(self.directory.clone(), &name)?;
        let file = self.directory.create_private_file(&name)?;
        let entry = OwnedEntry::new(self.directory.clone(), name, file.try_clone()?);
        Ok((
            Partial {
                entry,
                path,
                _owner: owner,
            },
            file.into(),
        ))
    }
}

pub(super) struct Partial {
    entry: OwnedEntry,
    pub(super) path: PathBuf,
    _owner: PartialOwner,
}

impl Partial {
    pub(super) fn publish(self, name: &std::ffi::OsStr) -> io::Result<()> {
        self.entry.publish(name)
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use async_std::io::WriteExt;

    #[async_std::test]
    async fn staged_download_declines_a_changed_destination() {
        let scope = tempfile::tempdir().unwrap();
        let path = scope.path().join("artifacts");
        std::fs::create_dir(&path).unwrap();
        let target = Destination::Path(path.join("library.jar")).bind().unwrap();
        let (partial, mut output) = target.stage().unwrap();
        output.write_all(b"downloaded bytes").await.unwrap();
        output.sync_all().await.unwrap();
        drop(output);

        std::fs::rename(&path, scope.path().join("previous")).unwrap();
        std::fs::create_dir(&path).unwrap();
        std::fs::write(path.join("library.jar"), b"keep these bytes").unwrap();

        assert!(partial.publish(&target.name).is_err());
        assert_eq!(
            std::fs::read(path.join("library.jar")).unwrap(),
            b"keep these bytes"
        );
        assert_eq!(std::fs::read_dir(&path).unwrap().count(), 1);
    }
}
