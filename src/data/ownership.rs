//! Lifetime exclusion for resources coordinated by the instance database.
use std::{
    fs, io,
    path::{Path, PathBuf},
    sync::Arc,
};

use super::layout::LauncherPaths;
use crate::{
    instance::content::error::ContentInitError,
    utils::{
        files::{self, directory::Directory, identity::FileIdentity},
        object_store::{ObjectStore, StoreLock},
    },
};

#[derive(Debug)]
pub(crate) struct LayoutLease {
    _root: StoreLock,
    _instances: Option<StoreLock>,
    _content: StoreLock,
    instances_path: PathBuf,
    instances_directory: Arc<Directory>,
}

impl LayoutLease {
    pub(crate) fn acquire(paths: &LauncherPaths) -> Result<Self, ContentInitError> {
        let root = paths.root().canonicalize()?;
        let root_lock = lock(&root)?;
        fs::create_dir_all(paths.instances())?;
        let instances = paths.instances().canonicalize()?;
        let instances_directory = Directory::open(&instances)?;
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
            instances_path: paths.instances().clone(),
            instances_directory,
        })
    }

    pub(crate) fn create_instance_directory(&self, path: &Path) -> io::Result<Arc<Directory>> {
        let relative = path
            .strip_prefix(&self.instances_path)
            .map_err(io::Error::other)?;
        self.instances_directory.verify_location()?;
        let instance = self.instances_directory.create_relative(relative)?;
        instance.verify_location()?;
        Ok(instance)
    }

    pub(crate) fn open_instance_directory(&self, path: &Path) -> io::Result<Arc<Directory>> {
        let relative = path
            .strip_prefix(&self.instances_path)
            .map_err(io::Error::other)?;
        self.instances_directory.verify_location()?;
        self.instances_directory.open_relative(relative)
    }
}

pub(crate) fn ensure_instance_directory(
    lease: Option<&LayoutLease>,
    path: &Path,
) -> io::Result<Arc<Directory>> {
    if let Some(lease) = lease {
        return lease.create_instance_directory(path);
    }
    fs::create_dir_all(path)?;
    Directory::open(path)
}

fn lock(directory: &Path) -> Result<StoreLock, ContentInitError> {
    let opened = Directory::open(directory)?;
    let file = opened.open_lock(std::ffi::OsStr::new(".launcher-content.lock"))?;
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
