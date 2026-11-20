//! Generation deletion is a replayable rename-to-quarantine protocol. Only
//! generations with a matching durable owner record are candidates; old/unknown
//! generations are never inferred from their directory name alone.
use super::{error::Result, retention};
use crate::{
    data::layout::LauncherPaths,
    utils::{
        files,
        object_store::catalog::{read_regular, RootCatalog},
        process, process_pin,
    },
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    ffi::OsStr,
    fs, io,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const RECORD: &str = ".sonata-generation.json";
const OWNER: &str = ".sonata-owner";
const QUEUE: &str = ".sonata-runtime-gc/generations";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct GenerationOwner {
    version: u32,
    instance: i64,
    generation: uuid::Uuid,
    database: Option<String>,
    pid: u32,
    created: u64,
}

impl GenerationOwner {
    pub(super) fn new(
        database: Option<String>,
        instance: i64,
        generation: uuid::Uuid,
    ) -> io::Result<Self> {
        Ok(Self {
            version: 1,
            instance,
            generation,
            database,
            pid: std::process::id(),
            created: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(io::Error::other)?
                .as_secs(),
        })
    }

    fn relative(&self) -> PathBuf {
        PathBuf::from("instances")
            .join(self.instance.to_string())
            .join(self.generation.to_string())
    }

    fn validate(&self) -> io::Result<()> {
        if self.version != 1
            || self.pid == 0
            || self
                .database
                .as_ref()
                .is_some_and(|identity| !files::hash_valid(identity, 32))
        {
            return Err(io::Error::other("invalid generation ownership record"));
        }
        Ok(())
    }

    #[cfg(test)]
    fn write(&self, root: &Path) -> io::Result<()> {
        let root = crate::utils::files::directory::Directory::ensure_path(root)?;
        self.write_in(&root)
    }

    pub(super) fn write_in(
        &self,
        root: &crate::utils::files::directory::Directory,
    ) -> io::Result<()> {
        let body = serde_json::to_vec(self).map_err(io::Error::other)?;
        root.write_bytes(std::ffi::OsStr::new(RECORD), &body)
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Intent {
    version: u32,
    owner: GenerationOwner,
}

/// The entire snapshot/rename/remove protocol is owned, including SQL locks.
pub(super) async fn collect(paths: Arc<LauncherPaths>, minimum_age: Duration) -> Result<usize> {
    async_std::task::spawn(async move {
        let snapshot = retention::snapshot(&paths).await?;
        tracing::debug!(
            roots = snapshot.generations.len(),
            hashes = snapshot.sha1.len(),
            "runtime generation roots gate validated"
        );
        #[cfg(test)]
        tests::checkpoint(snapshot.maintenance.root(), "snapshot").await;
        let worker = snapshot.maintenance.clone();
        let roots = snapshot.generations.clone();
        let owners: HashSet<_> = RootCatalog::owners(&worker)?
            .into_iter()
            .map(|owner| owner.identity)
            .collect();
        let removed = async_std::task::spawn_blocking(move || {
            collect_files(worker.directory(), &roots, &owners, minimum_age)
        })
        .await?;
        snapshot.databases.close().await;
        Ok(removed)
    })
    .await
}

fn directory(path: &Path) -> io::Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() => Ok(true),
        Ok(_) => Err(io::Error::other(
            "generation retention directory is not regular",
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

fn record(path: &Path) -> io::Result<GenerationOwner> {
    let owner: GenerationOwner = serde_json::from_slice(&read_regular(&path.join(RECORD), 4096)?)
        .map_err(io::Error::other)?;
    owner.validate()?;
    if process::read_owner(&path.join(OWNER)) != Some(owner.pid) {
        return Err(io::Error::other("generation owner PID mismatch"));
    }
    Ok(owner)
}

fn old_tree(path: &Path, minimum_age: Duration) -> io::Result<bool> {
    let mut directories = vec![path.to_owned()];
    let mut inspected = 0usize;
    while let Some(directory) = directories.pop() {
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            inspected += 1;
            if inspected > 1_000_000 {
                return Ok(false);
            }
            let metadata = fs::symlink_metadata(entry.path())?;
            if !metadata.is_dir() && !metadata.is_file() {
                return Ok(false);
            }
            if metadata.is_file()
                && metadata
                    .modified()?
                    .elapsed()
                    .map_or(true, |age| age < minimum_age)
            {
                return Ok(false);
            }
            if metadata.is_dir() {
                directories.push(entry.path());
            }
        }
    }
    Ok(true)
}

fn known_tree(path: &Path) -> io::Result<bool> {
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let name = entry.file_name();
        if !["maven", "work", ".sonata-processes", RECORD, OWNER]
            .iter()
            .any(|known| name == *known)
        {
            return Ok(false);
        }
        let kind = entry.file_type()?;
        if name == RECORD || name == OWNER {
            if !kind.is_file() {
                return Ok(false);
            }
        } else if !kind.is_dir() {
            return Ok(false);
        }
    }
    Ok(true)
}

fn eligible(
    path: &Path,
    owner: &GenerationOwner,
    owners: &HashSet<String>,
    minimum_age: Duration,
) -> io::Result<bool> {
    Ok(owner
        .database
        .as_ref()
        .is_some_and(|identity| owners.contains(identity))
        && !process::may_be_alive(owner.pid)
        && UNIX_EPOCH
            .checked_add(Duration::from_secs(owner.created))
            .and_then(|time| time.elapsed().ok())
            .is_some_and(|age| age >= minimum_age)
        && process_pin::allows_cleanup(path)?
        && known_tree(path)?
        && old_tree(path, minimum_age)?)
}

fn collect_files(
    base: &Arc<crate::utils::files::directory::Directory>,
    current: &HashSet<PathBuf>,
    owners: &HashSet<String>,
    age: Duration,
) -> io::Result<usize> {
    base.verify_location()?;
    let root = base.path();
    let queue = root.join(QUEUE);
    let queue_relative = Path::new(QUEUE);
    let mut queue_directory = base.open_optional(queue_relative)?;
    let mut pending = HashMap::new();
    if queue_directory.is_some() {
        for entry in fs::read_dir(&queue)? {
            let entry = entry?;
            let name = entry.file_name();
            let name = name
                .to_str()
                .ok_or_else(|| io::Error::other("unknown generation deletion entry"))?;
            if !name.ends_with(".json") {
                continue;
            } // data dirs are validated below
            let intent: Intent = serde_json::from_slice(&read_regular(&entry.path(), 4096)?)
                .map_err(io::Error::other)?;
            intent.owner.validate()?;
            if intent.version != 1
                || name != format!("{}.json", intent.owner.generation)
                || !intent
                    .owner
                    .database
                    .as_ref()
                    .is_some_and(|identity| owners.contains(identity))
            {
                return Err(io::Error::other("unknown generation deletion intent"));
            }
            pending.insert(intent.owner.generation, intent.owner);
        }
        // An unowned quarantine directory must not be silently ignored.
        for entry in fs::read_dir(&queue)? {
            let entry = entry?;
            let name = entry.file_name();
            let name = name
                .to_str()
                .ok_or_else(|| io::Error::other("unknown generation deletion entry"))?;
            if name.ends_with(".json") {
                continue;
            }
            let id = uuid::Uuid::parse_str(name).map_err(io::Error::other)?;
            if id.to_string() != name || !entry.file_type()?.is_dir() || !pending.contains_key(&id)
            {
                return Err(io::Error::other("unowned generation quarantine"));
            }
        }
    }
    let instances = root.join("instances");
    if directory(&instances)? {
        for instance in fs::read_dir(&instances)? {
            let instance = instance?;
            if !instance.file_type()?.is_dir() {
                continue;
            }
            for entry in fs::read_dir(instance.path())? {
                let entry = entry?;
                if !entry.file_type()?.is_dir() {
                    continue;
                }
                let relative = entry.path().strip_prefix(root).unwrap().to_owned();
                if current.contains(&relative) {
                    continue;
                }
                let Ok(owner) = record(&entry.path()) else {
                    continue;
                }; // historical/unknown preserved
                if relative != owner.relative() || !eligible(&entry.path(), &owner, owners, age)? {
                    continue;
                }
                if let Some(previous) = pending.get(&owner.generation) {
                    if previous != &owner {
                        return Err(io::Error::other(
                            "generation identity replaced during deletion",
                        ));
                    }
                } else {
                    if queue_directory.is_none() {
                        queue_directory = Some(base.create_relative(queue_relative)?);
                    }
                    let directory = queue_directory.as_ref().ok_or(io::ErrorKind::NotFound)?;
                    let intent = Intent {
                        version: 1,
                        owner: owner.clone(),
                    };
                    directory.write_bytes(
                        OsStr::new(&format!("{}.json", owner.generation)),
                        &serde_json::to_vec(&intent).map_err(io::Error::other)?,
                    )?;
                    checkpoint(root, "intent");
                    pending.insert(owner.generation, owner);
                }
            }
        }
    }
    let mut removed = 0;
    let Some(queue_directory) = queue_directory else {
        return Ok(0);
    };
    for owner in pending.into_values() {
        // Replay validates every original ancestor as well as quarantine. A
        // linked instance directory must not redirect a previously valid intent.
        directory(&instances)?;
        directory(&instances.join(owner.instance.to_string()))?;
        let relative = owner.relative();
        let relative_parent = relative.parent().ok_or(io::ErrorKind::InvalidInput)?;
        let generation = owner.generation.to_string();
        let name = OsStr::new(&generation);
        let original_parent = base.open_optional(relative_parent)?;
        let original = match &original_parent {
            Some(parent) => parent.open_optional(Path::new(name))?,
            None => None,
        };
        let mut tombstone = queue_directory.open_optional(Path::new(name))?;
        let intent = format!("{}.json", owner.generation);
        if current.contains(&owner.relative()) {
            if let Some(tombstone) = tombstone {
                if original.is_some() {
                    return Err(io::Error::other(
                        "new generation root conflicts with quarantine",
                    ));
                }
                if record(tombstone.path())? != owner
                    || !known_tree(tombstone.path())?
                    || !old_tree(tombstone.path(), Duration::ZERO)?
                {
                    return Err(io::Error::other(
                        "unknown quarantined bytes cannot be restored as a current generation",
                    ));
                }
                let parent = original_parent.ok_or(io::ErrorKind::NotFound)?;
                queue_directory.move_child_to(tombstone, &parent, name)?;
            }
            clear_intent(&queue_directory, OsStr::new(&intent), &owner)?;
            continue;
        }
        if let Some(original) = original {
            if tombstone.is_some() || record(original.path())? != owner {
                return Err(io::Error::other(
                    "generation replaced during deletion replay",
                ));
            }
            if !eligible(original.path(), &owner, owners, age)? {
                continue;
            }
            let parent = original_parent.ok_or(io::ErrorKind::NotFound)?;
            tombstone = Some(parent.move_child_to(original, &queue_directory, name)?);
            checkpoint(root, "rename");
        }
        if let Some(tombstone) = tombstone {
            tombstone.verify_location()?;
            match record(tombstone.path()) {
                Ok(record) if record == owner => {
                    if !eligible(tombstone.path(), &owner, owners, age)? {
                        continue;
                    }
                }
                Ok(_) => return Err(io::Error::other("quarantined generation owner changed")),
                Err(_) => {
                    // Ownership metadata is removed last. A crash here may only
                    // leave one or both exact metadata files, never outputs.
                    for entry in fs::read_dir(tombstone.path())? {
                        let entry = entry?;
                        if !entry.file_type()?.is_file()
                            || ![RECORD, OWNER]
                                .iter()
                                .any(|known| entry.file_name() == *known)
                        {
                            return Err(io::Error::other("unknown partial generation quarantine"));
                        }
                        if entry.file_name() == RECORD {
                            let record: GenerationOwner =
                                serde_json::from_slice(&read_regular(&entry.path(), 4096)?)
                                    .map_err(io::Error::other)?;
                            if record != owner {
                                return Err(io::Error::other("partial generation owner changed"));
                            }
                        } else if process::read_owner(&entry.path()) != Some(owner.pid) {
                            return Err(io::Error::other("partial generation PID changed"));
                        }
                    }
                }
            }
            for name in ["maven", "work", ".sonata-processes"] {
                if let Some(child) = tombstone.open_optional(Path::new(name))? {
                    tombstone.remove_child_tree(child)?;
                }
                checkpoint(root, "outputs");
            }
            for name in [RECORD, OWNER] {
                match tombstone.remove_regular(OsStr::new(name)) {
                    Ok(()) => {}
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error),
                }
                checkpoint(root, "metadata");
            }
            queue_directory.remove_empty_child(tombstone)?;
            removed += 1;
            checkpoint(root, "removed");
        }
        clear_intent(&queue_directory, OsStr::new(&intent), &owner)?;
        checkpoint(root, "cleaned");
    }
    Ok(removed)
}

fn clear_intent(
    queue: &crate::utils::files::directory::Directory,
    name: &OsStr,
    owner: &GenerationOwner,
) -> io::Result<()> {
    use crate::utils::object_store::catalog::read_open_regular;
    queue.remove_verified_file(name, |file| {
        let intent: Intent =
            serde_json::from_slice(&read_open_regular(file, 4096)?).map_err(io::Error::other)?;
        if intent.version != 1 || &intent.owner != owner {
            return Err(io::Error::other("generation deletion intent changed"));
        }
        Ok(())
    })
}

fn checkpoint(_root: &Path, _phase: &'static str) {
    #[cfg(test)]
    async_std::task::block_on(tests::checkpoint(_root, _phase));
}

/// A complete blob snapshot cannot guess unmaterialized inputs of a surviving
/// preparation. Unknown legacy directories, generations and quarantine block
/// this collector rather than interpreting a partial filesystem scan as roots.
pub(super) fn blob_roots(
    root: &Path,
    owners: &HashSet<String>,
    hashes: &mut HashSet<String>,
) -> io::Result<()> {
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        if ![
            "blobs",
            "instances",
            ".sonata-runtime-gc",
            ".sonata-processes",
            ".sonata-roots.json",
            ".sonata-access.lock",
        ]
        .iter()
        .any(|known| entry.file_name() == *known)
        {
            return Err(io::Error::other(
                "unknown library ownership blocks runtime blob collection",
            ));
        }
    }
    let queue = root.join(QUEUE);
    if directory(queue.parent().unwrap())? {
        for entry in fs::read_dir(queue.parent().unwrap())? {
            if entry?.file_name() != "generations" {
                return Err(io::Error::other("unknown runtime quarantine"));
            }
        }
        if directory(&queue)? && fs::read_dir(&queue)?.next().is_some() {
            return Err(io::Error::other(
                "pending generation deletion keeps runtime blobs",
            ));
        }
    }
    let instances = root.join("instances");
    if !directory(&instances)? {
        return Ok(());
    }
    for instance in fs::read_dir(instances)? {
        let instance = instance?;
        if !instance.file_type()?.is_dir() {
            return Err(io::Error::other("unknown library instance ownership"));
        }
        let name = instance.file_name();
        let name = name
            .to_str()
            .ok_or_else(|| io::Error::other("unknown library instance name"))?;
        if name
            .parse::<i64>()
            .map_or(true, |id| id.to_string() != name)
        {
            return Err(io::Error::other("noncanonical library instance ownership"));
        }
        for entry in fs::read_dir(instance.path())? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                return Err(io::Error::other("unknown generation ownership"));
            }
            let path = entry.path();
            let owner = record(&path)?;
            if path != root.join(owner.relative())
                || !owner
                    .database
                    .as_ref()
                    .is_some_and(|identity| owners.contains(identity))
                || process::may_be_alive(owner.pid)
                || !process_pin::allows_cleanup(&path)?
                || !known_tree(&path)?
                || !old_tree(&path, Duration::ZERO)?
                || UNIX_EPOCH
                    .checked_add(Duration::from_secs(owner.created))
                    .and_then(|time| time.elapsed().ok())
                    .is_none()
            {
                return Err(io::Error::other(
                    "live or unknown generation preparation keeps runtime blobs",
                ));
            }
            let mut directories = Vec::new();
            for name in ["maven", "work"] {
                let directory_path = path.join(name);
                if directory(&directory_path)? {
                    directories.push(directory_path);
                }
            }
            let mut inspected = 0usize;
            while let Some(directory) = directories.pop() {
                for entry in fs::read_dir(directory)? {
                    let entry = entry?;
                    inspected += 1;
                    if inspected > 1_000_000 {
                        return Err(io::Error::other("generation roots scan exceeds bound"));
                    }
                    let kind = entry.file_type()?;
                    if kind.is_dir() {
                        directories.push(entry.path());
                    } else if kind.is_file() {
                        hashes.insert(files::sha1_file(&entry.path())?);
                    } else {
                        return Err(io::Error::other("unknown generation file type"));
                    }
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
