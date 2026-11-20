//! Lifetime exclusion for resources coordinated by the instance database.
use std::{
    fs::{self, OpenOptions},
    io,
    path::{Path, PathBuf},
};

use super::layout::LauncherPaths;
use crate::{
    instance::content::error::ContentInitError,
    utils::{
        files::{self, identity::FileIdentity},
        object_store::{ObjectStore, StoreLock},
    },
};

#[derive(Debug)]
pub(crate) struct LayoutLease {
    _root: StoreLock,
    _instances: Option<StoreLock>,
    _content: StoreLock,
}

impl LayoutLease {
    pub(crate) fn acquire(paths: &LauncherPaths) -> Result<Self, ContentInitError> {
        let root = paths.root().canonicalize()?;
        let root_lock = lock(&root)?;
        fs::create_dir_all(paths.instances())?;
        let instances = paths.instances().canonicalize()?;
        let instances_lock = if root == instances
            || matches!(
                (FileIdentity::at(&root), FileIdentity::at(&instances)),
                (Ok(left), Ok(right)) if left == right
            ) {
            None
        } else {
            Some(lock(&instances)?)
        };
        let content = files::child(&root, Path::new("content-store"))?;
        let content = ObjectStore::new(content).lock_exclusive()?;
        Ok(Self {
            _root: root_lock,
            _instances: instances_lock,
            _content: content,
        })
    }
}

fn lock(directory: &Path) -> Result<StoreLock, ContentInitError> {
    let path = directory.join(".launcher-content.lock");
    match fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.is_file() => {}
        Ok(_) => {
            return Err(io::Error::other("launcher lifetime lock is not a regular file").into())
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)?;
    StoreLock::exclusive(file).map_err(|source| ContentInitError::Lock {
        path: directory.to_owned(),
        source,
    })
}

pub(crate) fn layout_key(paths: &LauncherPaths) -> io::Result<String> {
    let roots = [
        paths.root(),
        paths.instances(),
        paths.libraries(),
        paths.assets(),
        paths.java_runtimes(),
        paths.meta(),
    ]
    .into_iter()
    .map(|path| canonical_destination(path).map(|path| files::path_key(&path)))
    .collect::<io::Result<Vec<_>>>()?;
    serde_json::to_string(&(std::env::consts::OS, roots)).map_err(io::Error::other)
}

fn canonical_destination(path: &Path) -> io::Result<PathBuf> {
    let absolute = std::path::absolute(path)?;
    let mut suffix = Vec::new();
    for ancestor in absolute.ancestors() {
        match ancestor.canonicalize() {
            Ok(mut canonical) => {
                if !canonical.is_dir() {
                    return Err(io::Error::new(
                        io::ErrorKind::NotADirectory,
                        canonical.display().to_string(),
                    ));
                }
                for part in suffix.into_iter().rev() {
                    canonical.push(part);
                }
                return Ok(canonical);
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                // A dangling redirect does not describe an available root.
                if fs::symlink_metadata(ancestor).is_ok() {
                    return Err(error);
                }
                suffix.push(ancestor.file_name().ok_or(error)?);
            }
            Err(error) => return Err(error),
        }
    }
    Err(io::ErrorKind::NotFound.into())
}
