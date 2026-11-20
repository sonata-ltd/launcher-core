//! Managed installation ownership, never inferred from a UUID directory alone.
//! Old/unknown ownership is preserved; PID reuse conservatively defers recovery.
use super::{storage, ManagedJavaError, Result};
use crate::utils::{
    files::{self, directory::Directory, identity::FileIdentity},
    object_store::{
        catalog::{read_open_regular, read_regular},
        ObjectStore, StoreAccess, StoreMaintenance,
    },
    process,
};
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};
use std::{
    ffi::OsStr,
    io,
    path::{Path, PathBuf},
    sync::Arc,
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
    pub(super) fn valid(&self, operation: Uuid) -> bool {
        self.version == 1 && self.operation == operation && self.pid != 0
    }
    pub(super) fn local(&self, operation: Uuid) -> bool {
        self.valid(operation) && self.pid == std::process::id()
    }
    pub(super) fn dead(&self, operation: Uuid) -> bool {
        self.valid(operation) && !process::may_be_alive(self.pid)
    }
}

pub(super) const HOME_MARKER: &str = ".sonata-java-home.json";

/// Publication ownership is independent of the mutable operation owner. A later
/// validator may claim the operation but never adopts historical homes.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct HomeOwner {
    pub version: u32,
    pub operation: Uuid,
    pub package: String,
    pub database: Option<String>,
    pub pid: u32,
    pub created: u64,
    pub home: PathBuf,
    pub sha256: String,
    pub size: u64,
}

impl HomeOwner {
    pub(super) fn new(
        operation: Uuid,
        package: &super::ManagedJavaPackage,
        home: PathBuf,
        database: Option<String>,
    ) -> Result<Self> {
        let home = if home.as_os_str().is_empty() {
            home
        } else {
            crate::java::paths::portable(&home).map_err(storage)?.into()
        };
        Ok(Self {
            version: 1,
            operation,
            package: package.id.clone(),
            database,
            pid: std::process::id(),
            created: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(storage)?
                .as_secs(),
            home,
            sha256: package.sha256.to_ascii_lowercase(),
            size: package.size,
        })
    }

    pub(super) fn generation(&self) -> PathBuf {
        PathBuf::from("managed")
            .join(&self.package)
            .join(self.operation.to_string())
    }

    pub(super) fn validate(&self) -> Result<()> {
        if self.version != 1
            || self.pid == 0
            || self.size == 0
            || !files::hash_valid(&self.sha256, 64)
            || self
                .database
                .as_ref()
                .is_some_and(|identity| !files::hash_valid(identity, 32))
            || self.package.is_empty()
            || self.package.contains(['/', '\\', ':'])
            || self.package == "."
            || self.package == ".."
            || (!self.home.as_os_str().is_empty()
                && crate::java::paths::portable(&self.home).is_err())
            || self.created
                > SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map_err(storage)?
                    .as_secs()
        {
            return Err(storage("unknown managed home ownership"));
        }
        Ok(())
    }

    #[cfg(test)]
    pub(super) fn write(&self, generation: &Path) -> Result<()> {
        self.write_in(&Directory::open(generation).map_err(storage)?)
    }

    pub(super) fn write_in(&self, generation: &Arc<Directory>) -> Result<()> {
        self.validate()?;
        write_marker(generation, HOME_MARKER, self)
    }

    pub(super) fn read(generation: &Path) -> Result<Self> {
        let owner: Self = serde_json::from_slice(
            &read_regular(&generation.join(HOME_MARKER), 4096).map_err(storage)?,
        )
        .map_err(storage)?;
        owner.validate()?;
        Ok(owner)
    }
}

fn managed_store(root: &Path) -> Result<ObjectStore> {
    ObjectStore::beneath(root, Path::new(".managed")).map_err(storage)
}

pub(super) async fn access(root: &Path) -> Result<StoreAccess> {
    managed_store(root)?
        .access(&crate::utils::cancellation::Cancellation::new())
        .await
        .map_err(storage)
}

pub(super) fn maintenance(root: &Path) -> Result<StoreMaintenance> {
    managed_store(root)?.try_maintenance().map_err(|error| {
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
    ObjectStore::beneath(
        root,
        &PathBuf::from(".managed/operations/.locks").join(operation.to_string()),
    )
    .and_then(|store| store.try_maintenance())
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

pub(super) fn create_staging(
    root: &Path,
    operation: Uuid,
    owner: &Owner,
) -> Result<Arc<Directory>> {
    if !owner.local(operation) {
        return Err(storage("unowned managed staging writer"));
    }
    let parent = open_root(root)?
        .create_relative(Path::new(".managed/staging"))
        .map_err(storage)?;
    let directory = parent
        .create_private_child(OsStr::new(&operation.to_string()))
        .map_err(storage)?;
    #[cfg(test)]
    super::tests::ownership_checkpoint(root, "created");
    write_marker(&directory, MARKER, owner)?;
    #[cfg(test)]
    super::tests::ownership_checkpoint(root, "staging");
    Ok(directory)
}

pub(super) fn open_root(root: &Path) -> Result<Arc<Directory>> {
    Directory::open(&root.canonicalize().map_err(storage)?).map_err(storage)
}

fn write_marker(directory: &Arc<Directory>, name: &str, owner: &impl Serialize) -> Result<()> {
    directory
        .with_staged_file(
            |file| {
                serde_json::to_writer(&mut *file, owner).map_err(io::Error::other)?;
                file.sync_all()
            },
            |staged, ()| directory.publish(staged, directory, OsStr::new(name)),
        )
        .map_err(storage)
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
    let opened = (|| -> io::Result<_> {
        let base = Directory::open(&root.canonicalize()?)?;
        let parent = base.open_relative(Path::new(".managed/staging"))?;
        let staging = parent.open_relative(Path::new(&operation.to_string()))?;
        Ok((parent, staging))
    })();
    let (parent, staging) = match opened {
        Ok(opened) => opened,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(storage(error)),
    };
    let name = OsStr::new(MARKER);
    let marker = staging.open_file(name).map_err(storage)?;
    let actual: Owner = serde_json::from_slice(&read_open_regular(&marker, 4096).map_err(storage)?)
        .map_err(storage)?;
    if actual != *owner {
        return Err(storage("managed staging owner does not match operation"));
    }
    #[cfg(test)]
    super::tests::ownership_checkpoint(root, "cleanup");
    staging
        .verify_file(name, FileIdentity::from_handle(&marker).map_err(storage)?)
        .map_err(storage)?;
    drop(marker);
    parent.remove_marked_tree(staging, name).map_err(storage)
}
