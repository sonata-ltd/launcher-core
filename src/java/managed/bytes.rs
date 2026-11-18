//! Immutable archive bytes, separate from package provenance and extracted homes.
//! No published Java archive/home collection is enabled without domain roots and
//! process gates. An Archive retains CAS admission through blocking extraction.
use super::{storage, ManagedJavaError, ManagedJavaPackage, Result};
use crate::utils::{
    cancellation::Cancellation,
    files,
    object_store::{ObjectStore, StoreAccess},
};
use std::{
    io,
    path::{Path, PathBuf},
};

pub(super) struct Archive {
    pub path: PathBuf,
    _access: StoreAccess,
}

fn error(error: io::Error) -> ManagedJavaError {
    ManagedJavaError::caused_by(
        match error.kind() {
            io::ErrorKind::Interrupted => "cancelled",
            io::ErrorKind::InvalidData => "checksum",
            _ => "storage",
        },
        error,
    )
}

fn root(root: &Path) -> Result<PathBuf> {
    files::child(root, Path::new(".managed/bytes")).map_err(storage)
}

fn layout(root: &Path) -> io::Result<()> {
    for path in ["staging", "objects/sha256", "aliases/sha1"] {
        files::child(root, Path::new(path)).map_err(io::Error::other)?;
    }
    Ok(())
}

fn package(package: &ManagedJavaPackage) -> Result<()> {
    if !files::hash_valid(&package.sha256, 64) || package.size == 0 {
        return Err(ManagedJavaError::new(
            "checksum",
            "invalid Java archive SHA-256/size",
        ));
    }
    Ok(())
}

pub(super) async fn cached(
    root_path: &Path,
    wanted: &ManagedJavaPackage,
    cancel: &Cancellation,
) -> Result<Option<Archive>> {
    package(wanted)?;
    let access = ObjectStore::new(root(root_path)?)
        .access(cancel)
        .await
        .map_err(error)?;
    let wanted = wanted.clone();
    let cancel = cancel.clone();
    async_std::task::spawn_blocking(move || {
        layout(access.root()).map_err(error)?;
        let object = access.find_sha256(&wanted.sha256, &cancel).map_err(error)?;
        Ok(object
            .filter(|object| object.digests.size_bytes == wanted.size)
            .map(|object| Archive {
                path: object.path,
                _access: access,
            }))
    })
    .await
}

pub(super) async fn import(
    root_path: &Path,
    source: PathBuf,
    wanted: &ManagedJavaPackage,
    cancel: &Cancellation,
) -> Result<Archive> {
    package(wanted)?;
    let access = ObjectStore::new(root(root_path)?)
        .access(cancel)
        .await
        .map_err(error)?;
    let wanted = wanted.clone();
    let cancel = cancel.clone();
    async_std::task::spawn_blocking(move || {
        layout(access.root()).map_err(error)?;
        let object = access.import_checked(&source, &cancel, |digests| -> io::Result<()> {
            if !digests.sha256.eq_ignore_ascii_case(&wanted.sha256) || digests.size_bytes != wanted.size {
                return Err(io::Error::new(io::ErrorKind::InvalidData, format!("Java archive bytes differ: expected {} / {} bytes, actual {} / {} bytes", wanted.sha256, wanted.size, digests.sha256, digests.size_bytes)));
            }
            // Reject escaping sharded/alias ancestors before immutable publication.
            files::child(access.root(), &PathBuf::from("objects/sha256").join(&digests.sha256[..2]).join(&digests.sha256)).map_err(io::Error::other)?;
            files::child(access.root(), &PathBuf::from("aliases/sha1").join(&digests.sha1)).map_err(io::Error::other)?;
            Ok(())
        }).map_err(error)?;
        #[cfg(test)]
        super::tests::ownership_checkpoint(root_path_for_hook(access.root()), "archive-imported");
        Ok(Archive { path: object.path, _access: access })
    }).await
}

#[cfg(test)]
fn root_path_for_hook(root: &Path) -> &Path {
    root.parent().unwrap().parent().unwrap()
}
