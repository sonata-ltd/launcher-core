//! Append-only owner catalogue. It must precede the first managed bytes.
//! Missing historical coverage, unavailable owners and replacement databases
//! are blockers, never permission to collect from the one database still open.
use super::StoreMaintenance;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{self, Read},
    path::{Path, PathBuf},
};

const FILE: &str = ".sonata-roots.json";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct DatabaseOwner {
    pub identity: String,
    pub path: PathBuf,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RootCatalog {
    version: u32,
    complete: bool,
    owners: Vec<DatabaseOwner>,
}

impl RootCatalog {
    /// A known writer needs no exclusive catalogue rewrite. An incomplete
    /// catalogue already accounts for unknown owners by disabling collection.
    pub(crate) fn covers(root: &Path, owner: Option<&DatabaseOwner>) -> io::Result<bool> {
        let catalog = match Self::read(&root.join(FILE)) {
            Ok(catalog) => catalog,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(error),
        };
        Ok(match owner {
            Some(owner) => catalog.owners.contains(owner),
            None => !catalog.complete,
        })
    }

    pub(crate) fn attach(
        maintenance: &StoreMaintenance,
        owner: Option<DatabaseOwner>,
    ) -> io::Result<()> {
        let path = maintenance.root().join(FILE);
        let mut catalog = match Self::read(&path) {
            Ok(catalog) => catalog,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                // Never declare an old store completely covered just because
                // this is the first upgraded launcher to write a catalogue.
                let mut complete = true;
                for entry in fs::read_dir(maintenance.root())? {
                    let name = entry?.file_name();
                    if name != ".sonata-access.lock" && name != ".sonata-store.lock" {
                        complete = false;
                    }
                }
                Self {
                    version: 1,
                    complete,
                    owners: Vec::new(),
                }
            }
            Err(error) => return Err(error),
        };
        if let Some(owner) = owner {
            validate_owner(&owner)?;
            if !catalog.owners.contains(&owner) {
                catalog.owners.push(owner);
            }
        } else {
            // Ephemeral databases cannot be reopened after process exit.
            catalog.complete = false;
        }
        let body = serde_json::to_vec(&catalog).map_err(io::Error::other)?;
        crate::utils::files::atomic_write(&path, &body).map_err(io::Error::other)
    }

    pub(crate) fn owners(maintenance: &StoreMaintenance) -> io::Result<Vec<DatabaseOwner>> {
        let catalog = Self::read(&maintenance.root().join(FILE))?;
        if !catalog.complete || catalog.owners.is_empty() {
            return Err(io::Error::other(
                "retention catalogue has unknown or ephemeral owners",
            ));
        }
        Ok(catalog.owners)
    }

    fn read(path: &Path) -> io::Result<Self> {
        let body = read_regular(path, 1024 * 1024)?;
        let catalog: Self = serde_json::from_slice(&body).map_err(io::Error::other)?;
        if catalog.version != 1 || catalog.owners.len() > 1024 {
            return Err(io::Error::other(
                "unsupported or oversized retention catalogue",
            ));
        }
        for owner in &catalog.owners {
            validate_owner(owner)?;
        }
        Ok(catalog)
    }
}

fn validate_owner(owner: &DatabaseOwner) -> io::Result<()> {
    if !crate::utils::files::hash_valid(&owner.identity, 32) || !owner.path.is_absolute() {
        return Err(io::Error::other("invalid retention database owner"));
    }
    Ok(())
}

pub(crate) fn read_regular(path: &Path, limit: u64) -> io::Result<Vec<u8>> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_file() || metadata.len() > limit {
        return Err(io::Error::other(
            "retention metadata is not a bounded regular file",
        ));
    }
    let mut body = Vec::new();
    fs::File::open(path)?
        .take(limit + 1)
        .read_to_end(&mut body)?;
    if body.len() as u64 > limit {
        return Err(io::Error::other("retention metadata grew beyond limit"));
    }
    Ok(body)
}
