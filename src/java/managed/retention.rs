//! Conservative Java domain roots, protected by managed admission and every
//! catalogued database write lock. Inspection never deletes homes or archives.
use super::{ownership::HomeOwner, recovery, storage, ManagedJavaError, Result};
use crate::{
    data::db::{retention::LockedDatabases, Database},
    utils::{
        files,
        object_store::{catalog::RootCatalog, ObjectStore, StoreMaintenance},
        process_pin,
    },
};
use std::{
    collections::{HashMap, HashSet},
    fs, io,
    path::{Path, PathBuf},
    sync::Arc,
};
use uuid::Uuid;

fn scope(root: &Path) -> Result<ObjectStore> {
    ObjectStore::beneath(root, Path::new(".managed")).map_err(storage)
}

pub(crate) async fn attach(database: Option<Arc<Database>>, root: PathBuf) -> Result<()> {
    async_std::task::spawn(async move {
        let store = scope(&root)?;
        let access = store.try_access().map_err(|error| {
            ManagedJavaError::new(
                if error.kind() == io::ErrorKind::WouldBlock {
                    "busy"
                } else {
                    "storage"
                },
                error,
            )
        })?;
        let owner = match &database {
            Some(db) => db.retention_owner().await.ok().flatten(),
            None => None,
        };
        if RootCatalog::covers(access.root(), owner.as_ref()).map_err(storage)? {
            return Ok(());
        }
        drop(access);
        let maintenance = store.try_maintenance().map_err(|error| {
            ManagedJavaError::new(
                if error.kind() == io::ErrorKind::WouldBlock {
                    "busy"
                } else {
                    "storage"
                },
                error,
            )
        })?;
        // Homes live beside the metadata scope. A first upgraded catalogue must
        // not declare pre-existing/unknown placements completely covered.
        if !maintenance.root().join(".sonata-roots.json").exists() {
            match fs::symlink_metadata(root.join("managed")) {
                Ok(_) => RootCatalog::attach(&maintenance, None).map_err(storage)?,
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(storage(error)),
            }
        }
        let owner = match &database {
            Some(db) => db.retention_owner().await.ok().flatten(),
            None => None,
        };
        RootCatalog::attach(&maintenance, owner).map_err(storage)
    })
    .await
}

pub(crate) struct Snapshot {
    pub homes: HashSet<PathBuf>,
    pub archives: HashSet<String>,
    pub databases: LockedDatabases,
    pub maintenance: Arc<StoreMaintenance>,
    pub archive_maintenance: Arc<StoreMaintenance>,
}

impl Snapshot {
    pub(crate) async fn close(self) {
        let _managed = self.maintenance;
        let _archives = self.archive_maintenance;
        self.databases.close().await;
    }
}

#[cfg(test)]
pub(crate) async fn inspect(root: PathBuf) -> Result<(usize, usize)> {
    async_std::task::spawn(async move {
        let snapshot = snapshot(&root).await?;
        #[cfg(test)]
        tests::checkpoint(snapshot.maintenance.root().parent().unwrap(), "snapshot").await;
        let count = (snapshot.homes.len(), snapshot.archives.len());
        snapshot.close().await;
        Ok(count)
    })
    .await
}

pub(crate) async fn snapshot(configured: &Path) -> Result<Snapshot> {
    let maintenance = Arc::new(super::ownership::maintenance(configured)?);
    let root = maintenance
        .root()
        .parent()
        .ok_or_else(|| storage("missing managed Java parent"))?;
    if !process_pin::allows_cleanup(maintenance.root()).map_err(storage)? {
        return Err(storage("live or unknown managed Java process pin"));
    }
    let archive_maintenance = Arc::new(
        ObjectStore::beneath(root, Path::new(".managed/bytes"))
            .and_then(|store| store.try_maintenance())
            .map_err(storage)?,
    );
    let owners: HashSet<_> = RootCatalog::owners(&maintenance)
        .map_err(storage)?
        .into_iter()
        .map(|owner| owner.identity)
        .collect();
    let mut databases = LockedDatabases::open(maintenance.clone())
        .await
        .map_err(storage)?;
    let mut homes = HashSet::new();
    let mut archives = HashSet::new();
    for tx in &mut databases.transactions {
        // All registrations retain their placements, including rows currently
        // unselected by instance/global settings, and manual/discovered paths.
        let rows: Vec<(String, String, String)> =
            sqlx::query_as("SELECT exec_path,home_path,source FROM java_runtimes")
                .fetch_all(&mut **tx)
                .await
                .map_err(storage)?;
        for (exec, home, source) in rows {
            if !["manual", "discovered", "managed"].contains(&source.as_str()) {
                return Err(storage("unknown Java SQL source schema"));
            }
            let exec = Path::new(&exec);
            let home = Path::new(&home);
            let resolved_home = home.canonicalize().ok();
            let resolved_exec = exec.canonicalize().ok().or_else(|| {
                let relative = exec.strip_prefix(home).ok()?;
                let canonical_home = resolved_home.as_ref()?;
                files::child(canonical_home, relative).ok()
            });
            let internal = source == "managed"
                || exec.starts_with(root)
                || home.starts_with(root)
                || resolved_exec
                    .as_ref()
                    .is_some_and(|exec| exec.starts_with(root))
                || resolved_home
                    .as_ref()
                    .is_some_and(|home| home.starts_with(root))
                || !exec.is_absolute()
                || !home.is_absolute();
            if !internal {
                continue;
            }
            let (exec, home) = crate::java::storage::paths::pair(root, exec, home)
                .or_else(|| {
                    crate::java::storage::paths::pair(
                        root,
                        resolved_exec.as_deref()?,
                        resolved_home.as_deref()?,
                    )
                })
                .ok_or_else(|| storage("unknown Java SQL placement ownership"))?;
            let placement = generation(Path::new(&home))?;
            if generation(Path::new(&exec))? != placement {
                return Err(storage("conflicting Java SQL generation pair"));
            }
            homes.insert(placement);
        }
    }
    for scratch in [".managed/staging", ".managed/bytes/staging"] {
        let scratch = files::child(root, Path::new(scratch)).map_err(storage)?;
        if directory(&scratch)? && fs::read_dir(scratch).map_err(storage)?.next().is_some() {
            return Err(storage("pending or unknown managed Java scratch"));
        }
    }
    let mut records = HashMap::new();
    let operations = files::child(root, Path::new(".managed/operations")).map_err(storage)?;
    if directory(&operations)? {
        for entry in fs::read_dir(&operations).map_err(storage)? {
            let entry = entry.map_err(storage)?;
            if entry.file_name() == ".locks" && entry.file_type().map_err(storage)?.is_dir() {
                continue;
            }
            let record = recovery::decode(root, &entry.path())?;
            let id = record.snapshot.id;
            if record.snapshot.stage.is_active()
                || !record.owner.as_ref().is_some_and(|owner| owner.valid(id))
                || record.snapshot.created_at > chrono::Utc::now()
                || !files::hash_valid(&record.snapshot.package.sha256, 64)
                || record.snapshot.package.size == 0
            {
                return Err(storage("live or unknown managed Java operation retention"));
            }
            archives.insert(record.snapshot.package.sha256.to_ascii_lowercase());
            if let Some(home) = &record.home {
                files::child(root, home).map_err(storage)?;
                let placement = generation(home)?;
                if placement
                    != PathBuf::from("managed")
                        .join(&record.snapshot.package.id)
                        .join(id.to_string())
                {
                    return Err(storage("managed operation home identity mismatch"));
                }
                homes.insert(placement);
            }
            records.insert(id, record);
        }
    }
    // Every surviving placement retains its source archive. Unmarked legacy,
    // mismatched/unknown/future ownership blocks the entire domain, not just a
    // seemingly unreferenced digest. Operation absence is not adoption proof.
    let managed = files::child(root, Path::new("managed")).map_err(storage)?;
    if directory(&managed)? {
        for package in fs::read_dir(&managed).map_err(storage)? {
            let package = package.map_err(storage)?;
            if !package.file_type().map_err(storage)?.is_dir() {
                return Err(storage("unknown managed Java package layout"));
            }
            for entry in fs::read_dir(package.path()).map_err(storage)? {
                let entry = entry.map_err(storage)?;
                if !entry.file_type().map_err(storage)?.is_dir() {
                    return Err(storage("unknown managed Java generation layout"));
                }
                let owner = HomeOwner::read(&entry.path())?;
                if entry.path().strip_prefix(root).map_err(storage)? != owner.generation()
                    || !owner
                        .database
                        .as_ref()
                        .is_some_and(|identity| owners.contains(identity))
                {
                    return Err(storage("uncatalogued or mismatched managed home owner"));
                }
                let home = owner.generation().join(&owner.home);
                files::child(root, &home).map_err(storage)?;
                if let Some(record) = records.get(&owner.operation) {
                    if record.home.as_ref() != Some(&home)
                        || !record
                            .snapshot
                            .package
                            .sha256
                            .eq_ignore_ascii_case(&owner.sha256)
                        || record.snapshot.package.size != owner.size
                    {
                        return Err(storage("managed home marker differs from operation"));
                    }
                }
                archives.insert(owner.sha256.to_ascii_lowercase());
            }
        }
    }
    Ok(Snapshot {
        homes,
        archives,
        databases,
        maintenance,
        archive_maintenance,
    })
}

fn directory(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() => Ok(true),
        Ok(_) => Err(storage("Java retention directory is not regular")),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(storage(error)),
    }
}

fn generation(path: &Path) -> Result<PathBuf> {
    crate::java::paths::portable(path).map_err(storage)?;
    let parts: Vec<_> = path.iter().collect();
    if parts.len() < 3 || parts[0] != "managed" {
        return Err(storage("unknown managed Java generation layout"));
    }
    let id = parts[2]
        .to_str()
        .ok_or_else(|| storage("non-Unicode Java generation"))?;
    if Uuid::parse_str(id).map_err(storage)?.to_string() != id {
        return Err(storage("noncanonical Java generation identity"));
    }
    Ok(PathBuf::from("managed").join(parts[1]).join(id))
}

#[cfg(test)]
mod tests;
