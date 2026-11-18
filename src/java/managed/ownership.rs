//! Managed installation ownership, never inferred from a UUID directory alone.
//! Old/unknown ownership is preserved; PID reuse conservatively defers recovery.
use super::{storage, ManagedJavaError, Result};
use crate::utils::{
    files,
    object_store::{catalog::read_regular, ObjectStore, StoreAccess, StoreMaintenance},
    process,
};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
};
use uuid::Uuid;

const MARKER: &str = ".sonata-owner.json";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct Owner {
    version: u32,
    operation: Uuid,
    pid: u32,
}

impl Owner {
    pub(super) fn new(operation: Uuid) -> Self {
        Self {
            version: 1,
            operation,
            pid: std::process::id(),
        }
    }
    fn valid(&self, operation: Uuid) -> bool {
        self.version == 1 && self.operation == operation && self.pid != 0
    }
    pub(super) fn local(&self, operation: Uuid) -> bool {
        self.valid(operation) && self.pid == std::process::id()
    }
    pub(super) fn dead(&self, operation: Uuid) -> bool {
        self.valid(operation) && !process::may_be_alive(self.pid)
    }
}

fn managed_root(root: &Path) -> Result<PathBuf> {
    files::child(root, Path::new(".managed")).map_err(storage)
}

pub(super) async fn access(root: &Path) -> Result<StoreAccess> {
    ObjectStore::new(managed_root(root)?)
        .access(&crate::utils::cancellation::Cancellation::new())
        .await
        .map_err(storage)
}

pub(super) fn maintenance(root: &Path) -> Result<StoreMaintenance> {
    ObjectStore::new(managed_root(root)?)
        .try_maintenance()
        .map_err(|error| {
            ManagedJavaError::new(
                if error.kind() == std::io::ErrorKind::WouldBlock {
                    "busy"
                } else {
                    "storage"
                },
                error,
            )
        })
}

/// Shared admission allows independent operations, not concurrent writes to one
/// operation record. Hold this exclusive scope through the owned task's finish.
pub(super) fn operation(root: &Path, operation: Uuid) -> Result<StoreMaintenance> {
    let scope = files::child(
        root,
        &PathBuf::from(".managed/operations/.locks").join(operation.to_string()),
    )
    .map_err(storage)?;
    ObjectStore::new(scope).try_maintenance().map_err(|error| {
        ManagedJavaError::new(
            if error.kind() == std::io::ErrorKind::WouldBlock {
                "busy"
            } else {
                "storage"
            },
            error,
        )
    })
}

fn staging(root: &Path, operation: Uuid) -> Result<PathBuf> {
    files::child(
        root,
        &PathBuf::from(".managed/staging").join(operation.to_string()),
    )
    .map_err(storage)
}

pub(super) fn create_staging(root: &Path, operation: Uuid, owner: &Owner) -> Result<PathBuf> {
    if !owner.local(operation) {
        return Err(storage("unowned managed staging writer"));
    }
    let path = staging(root, operation)?;
    fs::create_dir_all(path.parent().unwrap()).map_err(storage)?;
    fs::create_dir(&path).map_err(storage)?;
    #[cfg(test)]
    super::tests::ownership_checkpoint(root, "created");
    let mut file = tempfile::NamedTempFile::new_in(&path).map_err(storage)?;
    serde_json::to_writer(&mut file, owner).map_err(storage)?;
    file.as_file().sync_all().map_err(storage)?;
    file.persist_noclobber(path.join(MARKER)).map_err(storage)?;
    #[cfg(test)]
    super::tests::ownership_checkpoint(root, "staging");
    Ok(path)
}

pub(super) fn cleanup_staging(
    root: &Path,
    operation: Uuid,
    owner: &Owner,
    recovery: bool,
) -> Result<()> {
    if !(if recovery {
        owner.dead(operation)
    } else {
        owner.local(operation)
    }) {
        return Err(storage("live or unknown managed staging owner"));
    }
    let path = staging(root, operation)?;
    match fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.is_dir() => {}
        Ok(_) => return Err(storage("managed staging is not regular")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(storage(error)),
    }
    let actual: Owner =
        serde_json::from_slice(&read_regular(&path.join(MARKER), 4096).map_err(storage)?)
            .map_err(storage)?;
    if actual != *owner {
        return Err(storage("managed staging owner does not match operation"));
    }
    #[cfg(test)]
    super::tests::ownership_checkpoint(root, "cleanup");
    fs::remove_dir_all(path).map_err(storage)
}
