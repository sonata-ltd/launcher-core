//! Managed published-home quarantine and immutable archive collection. Every
//! replay reacquires complete SQL roots and managed/archive maintenance; neither
//! package hints nor a missing operation/registration proves absent ownership.
use super::{
    ownership::{HomeOwner, HOME_MARKER},
    retention, storage, Result,
};
use crate::utils::{
    files::{self, directory::Directory},
    object_store::{
        catalog::{read_regular, RootCatalog},
        collection::{Collection, DeleteIntent, Roots},
    },
    process, process_pin,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, UNIX_EPOCH},
};
use uuid::Uuid;

const QUEUE: &str = ".managed/.sonata-java-gc/homes";

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Intent {
    version: u32,
    owner: HomeOwner,
}

pub(crate) async fn collect(root: PathBuf, age: Duration) -> Result<(usize, usize)> {
    async_std::task::spawn(async move {
        let homes = collect_homes(&root, age).await?;
        let archives = collect_archives(&root, age).await?;
        #[cfg(test)]
        tests::checkpoint(&root, "collected").await;
        Ok((homes, archives))
    })
    .await
}

async fn collect_homes(root: &Path, age: Duration) -> Result<usize> {
    let snapshot = retention::snapshot(root).await?;
    #[cfg(test)]
    tests::checkpoint(snapshot.maintenance.root().parent().unwrap(), "snapshot").await;
    let managed = snapshot.maintenance.clone();
    let archives = snapshot.archive_maintenance.clone();
    let rooted = snapshot.homes.clone();
    let owners: HashSet<_> = RootCatalog::owners(&managed)
        .map_err(storage)?
        .into_iter()
        .map(|owner| owner.identity)
        .collect();
    let result = async_std::task::spawn_blocking(move || {
        let _archives = archives;
        let root = managed
            .root()
            .parent()
            .ok_or_else(|| storage("missing Java root"))?;
        layout(root)?;
        let base = Directory::open(root).map_err(storage)?;
        collect_home_files(&base, managed.directory(), &rooted, &owners, age)
    })
    .await;
    snapshot.close().await;
    result
}

async fn collect_archives(root: &Path, age: Duration) -> Result<usize> {
    let snapshot = retention::snapshot(root).await?;
    let managed = snapshot.maintenance.clone();
    let collection = Collection::new(snapshot.archive_maintenance.clone()).map_err(storage)?;
    #[cfg(test)]
    let collection = collection.with_test_hook(tests::archive_checkpoint);
    let roots = Roots {
        sha256: snapshot.archives.clone(),
        ..Roots::default()
    };
    let result = async_std::task::spawn_blocking(move || {
        let root = managed
            .root()
            .parent()
            .ok_or_else(|| storage("missing Java root"))?;
        layout(root)?;
        let queue = files::child(root, Path::new(QUEUE)).map_err(storage)?;
        if directory(&queue)? && fs::read_dir(&queue).map_err(storage)?.next().is_some() {
            return Err(storage("pending Java home quarantine retains archives"));
        }
        let plan = collection.plan(&roots, age).map_err(storage)?;
        let mut removed = 0;
        for intent in &plan {
            removed += usize::from(collection.unlink(intent).map_err(storage)?);
        }
        collection.clear_finished_intents(&plan).map_err(storage)?;
        checkpoint(root, "archive-cleaned");
        Ok(removed)
    })
    .await;
    snapshot.close().await;
    result
}

fn directory(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() => Ok(true),
        Ok(_) => Err(storage("Java collection directory is not regular")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(storage(error)),
    }
}

/// Reject links/special files/future times throughout the scope before writing
/// any deletion intent. A newer regular output merely defers its generation.
fn old_tree(path: &Path, age: Duration) -> Result<bool> {
    let mut todo = vec![path.to_owned()];
    let mut old = true;
    let mut inspected = 0;
    while let Some(path) = todo.pop() {
        for entry in fs::read_dir(path).map_err(storage)? {
            let entry = entry.map_err(storage)?;
            inspected += 1;
            if inspected > 1_000_000 {
                return Err(storage("Java tree scan exceeds bound"));
            }
            let metadata = fs::symlink_metadata(entry.path()).map_err(storage)?;
            if metadata.is_dir() {
                todo.push(entry.path());
            } else if metadata.is_file() {
                old &= metadata
                    .modified()
                    .map_err(storage)?
                    .elapsed()
                    .map_err(storage)?
                    >= age;
            } else {
                return Err(storage("unknown managed Java output type"));
            }
        }
    }
    Ok(old)
}

fn eligible(
    path: &Path,
    owner: &HomeOwner,
    owners: &HashSet<String>,
    age: Duration,
) -> Result<bool> {
    owner.validate()?;
    Ok(owner
        .database
        .as_ref()
        .is_some_and(|identity| owners.contains(identity))
        && !process::may_be_alive(owner.pid)
        && UNIX_EPOCH
            .checked_add(Duration::from_secs(owner.created))
            .and_then(|time| time.elapsed().ok())
            .is_some_and(|elapsed| elapsed >= age)
        && process_pin::allows_cleanup(path).map_err(storage)?
        && old_tree(path, age)?)
}

fn pending(root: &Path, owners: &HashSet<String>) -> Result<HashMap<Uuid, HomeOwner>> {
    let queue = files::child(root, Path::new(QUEUE)).map_err(storage)?;
    let mut pending = HashMap::new();
    if !directory(&queue)? {
        return Ok(pending);
    }
    for entry in fs::read_dir(&queue).map_err(storage)? {
        let entry = entry.map_err(storage)?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| storage("non-Unicode Java quarantine name"))?;
        if !name.ends_with(".json") {
            continue;
        }
        let intent: Intent =
            serde_json::from_slice(&read_regular(&entry.path(), 4096).map_err(storage)?)
                .map_err(storage)?;
        intent.owner.validate()?;
        if intent.version != 1
            || name != format!("{}.json", intent.owner.operation)
            || !intent
                .owner
                .database
                .as_ref()
                .is_some_and(|identity| owners.contains(identity))
        {
            return Err(storage("unknown Java home deletion intent"));
        }
        pending.insert(intent.owner.operation, intent.owner);
    }
    for entry in fs::read_dir(&queue).map_err(storage)? {
        let entry = entry.map_err(storage)?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| storage("non-Unicode Java quarantine name"))?;
        if name.ends_with(".json") {
            continue;
        }
        let id = Uuid::parse_str(&name).map_err(storage)?;
        if id.to_string() != name
            || !entry.file_type().map_err(storage)?.is_dir()
            || !pending.contains_key(&id)
        {
            return Err(storage("unowned Java home quarantine"));
        }
        let owner = pending.get(&id).unwrap();
        if entry
            .path()
            .join(HOME_MARKER)
            .try_exists()
            .map_err(storage)?
        {
            if HomeOwner::read(&entry.path())? != *owner {
                return Err(storage("Java quarantine owner changed"));
            }
            old_tree(&entry.path(), Duration::ZERO)?;
        } else if fs::read_dir(entry.path())
            .map_err(storage)?
            .next()
            .is_some()
        {
            return Err(storage("unowned partial Java quarantine outputs"));
        }
    }
    Ok(pending)
}

fn collect_home_files(
    base: &Arc<Directory>,
    control: &Arc<Directory>,
    rooted: &HashSet<PathBuf>,
    owners: &HashSet<String>,
    age: Duration,
) -> Result<usize> {
    let root = base.path();
    base.verify_location().map_err(storage)?;
    control.verify_location().map_err(storage)?;
    let queue_relative = Path::new(QUEUE).strip_prefix(".managed").map_err(storage)?;
    let mut queue_directory = control.open_optional(queue_relative).map_err(storage)?;
    let mut pending = pending(root, owners)?;
    let managed = files::child(root, Path::new("managed")).map_err(storage)?;
    if directory(&managed)? {
        for package in fs::read_dir(&managed).map_err(storage)? {
            let package = package.map_err(storage)?;
            for entry in fs::read_dir(package.path()).map_err(storage)? {
                let entry = entry.map_err(storage)?;
                let owner = HomeOwner::read(&entry.path())?;
                if rooted.contains(&owner.generation())
                    || !eligible(&entry.path(), &owner, owners, age)?
                {
                    continue;
                }
                if let Some(previous) = pending.get(&owner.operation) {
                    if previous != &owner {
                        return Err(storage("Java home replaced during deletion"));
                    }
                } else {
                    if queue_directory.is_none() {
                        queue_directory =
                            Some(control.create_relative(queue_relative).map_err(storage)?);
                    }
                    let queue = queue_directory
                        .as_ref()
                        .ok_or_else(|| storage("missing Java quarantine"))?;
                    queue
                        .write_bytes(
                            OsStr::new(&format!("{}.json", owner.operation)),
                            &serde_json::to_vec(&Intent {
                                version: 1,
                                owner: owner.clone(),
                            })
                            .map_err(storage)?,
                        )
                        .map_err(storage)?;
                    checkpoint(root, "home-intent");
                    pending.insert(owner.operation, owner);
                }
            }
        }
    }
    let mut removed = 0;
    let Some(queue) = queue_directory else {
        return Ok(0);
    };
    for owner in pending.into_values() {
        let relative = owner.generation();
        let relative_parent = relative
            .parent()
            .ok_or_else(|| storage("missing Java generation parent"))?;
        let generation = owner.operation.to_string();
        let name = OsStr::new(&generation);
        let original_parent = base.open_optional(relative_parent).map_err(storage)?;
        let original = match &original_parent {
            Some(parent) => parent.open_optional(Path::new(name)).map_err(storage)?,
            None => None,
        };
        let mut quarantine = queue.open_optional(Path::new(name)).map_err(storage)?;
        let intent = format!("{}.json", owner.operation);
        if rooted.contains(&owner.generation()) {
            if let Some(quarantine) = quarantine {
                if original.is_some() || HomeOwner::read(quarantine.path())? != owner {
                    return Err(storage("rooted Java home conflicts with quarantine"));
                }
                old_tree(quarantine.path(), Duration::ZERO)?;
                let parent = match original_parent {
                    Some(parent) => parent,
                    None => base.create_relative(relative_parent).map_err(storage)?,
                };
                queue
                    .move_child_to(quarantine, &parent, name)
                    .map_err(storage)?;
            }
            clear_intent(&queue, OsStr::new(&intent), &owner)?;
            continue;
        }
        if let Some(original) = original {
            if quarantine.is_some() || HomeOwner::read(original.path())? != owner {
                return Err(storage("Java home identity changed before quarantine"));
            }
            if !eligible(original.path(), &owner, owners, age)? {
                continue;
            }
            let parent =
                original_parent.ok_or_else(|| storage("missing Java generation parent"))?;
            quarantine = Some(
                parent
                    .move_child_to(original, &queue, name)
                    .map_err(storage)?,
            );
            checkpoint(root, "home-rename");
        }
        if let Some(quarantine) = quarantine {
            quarantine.verify_location().map_err(storage)?;
            if quarantine
                .path()
                .join(HOME_MARKER)
                .try_exists()
                .map_err(storage)?
            {
                if HomeOwner::read(quarantine.path())? != owner {
                    return Err(storage("quarantined Java home identity changed"));
                }
                if !eligible(quarantine.path(), &owner, owners, age)? {
                    continue;
                }
                for entry in fs::read_dir(quarantine.path()).map_err(storage)? {
                    let entry = entry.map_err(storage)?;
                    if entry.file_name() == HOME_MARKER {
                        continue;
                    }
                    let kind = entry.file_type().map_err(storage)?;
                    let name = entry.file_name();
                    if kind.is_dir() {
                        let child = quarantine
                            .open_relative(Path::new(&name))
                            .map_err(storage)?;
                        quarantine.remove_child_tree(child).map_err(storage)?;
                    } else if kind.is_file() {
                        quarantine.remove_regular(&name).map_err(storage)?;
                    } else {
                        return Err(storage("unknown Java quarantine output"));
                    }
                }
                checkpoint(root, "home-outputs");
                quarantine
                    .remove_regular(OsStr::new(HOME_MARKER))
                    .map_err(storage)?;
                checkpoint(root, "home-metadata");
            } else {
                if fs::read_dir(quarantine.path())
                    .map_err(storage)?
                    .next()
                    .is_some()
                {
                    return Err(storage("unowned partial Java quarantine"));
                }
                if !eligible(quarantine.path(), &owner, owners, age)? {
                    continue;
                }
            }
            queue.remove_empty_child(quarantine).map_err(storage)?;
            removed += 1;
            checkpoint(root, "home-removed");
        }
        clear_intent(&queue, OsStr::new(&intent), &owner)?;
        checkpoint(root, "home-cleaned");
    }
    Ok(removed)
}

fn clear_intent(queue: &Directory, name: &OsStr, owner: &HomeOwner) -> Result<()> {
    use crate::utils::object_store::catalog::read_open_regular;
    use std::io;

    queue
        .remove_verified_file(name, |file| {
            let intent: Intent = serde_json::from_slice(&read_open_regular(file, 4096)?)
                .map_err(io::Error::other)?;
            if intent.version != 1 || &intent.owner != owner {
                return Err(io::Error::other("Java home deletion intent changed"));
            }
            Ok(())
        })
        .map_err(storage)
}

/// Domain-wide layout preflight; immutable objects are still freshly verified
/// by Collection, and aliases remain hints rather than roots.
fn layout(root: &Path) -> Result<()> {
    entries(root, &["managed", ".managed"], &[])?;
    let scope = root.join(".managed");
    entries(
        &scope,
        &[
            "operations",
            "staging",
            "bytes",
            ".sonata-processes",
            ".sonata-java-gc",
        ],
        &[".sonata-access.lock", ".sonata-roots.json"],
    )?;
    let queue_parent = scope.join(".sonata-java-gc");
    entries(&queue_parent, &["homes"], &[])?;
    let locks = scope.join("operations/.locks");
    if directory(&locks)? {
        for entry in fs::read_dir(&locks).map_err(storage)? {
            let entry = entry.map_err(storage)?;
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| storage("non-Unicode Java operation lock"))?;
            if Uuid::parse_str(&name).map_or(true, |id| id.to_string() != name)
                || !entry.file_type().map_err(storage)?.is_dir()
            {
                return Err(storage("unknown Java operation lock scope"));
            }
            entries(&entry.path(), &[], &[".sonata-access.lock"])?;
        }
    }
    let managed = root.join("managed");
    if directory(&managed)? {
        for package in fs::read_dir(managed).map_err(storage)? {
            let package = package.map_err(storage)?;
            if !package.file_type().map_err(storage)?.is_dir() {
                return Err(storage("unknown Java package layout"));
            }
            for entry in fs::read_dir(package.path()).map_err(storage)? {
                let entry = entry.map_err(storage)?;
                if !entry.file_type().map_err(storage)?.is_dir() {
                    return Err(storage("unknown Java generation layout"));
                }
                old_tree(&entry.path(), Duration::ZERO)?;
            }
        }
    }
    let bytes = scope.join("bytes");
    entries(
        &bytes,
        &["objects", "aliases", "staging", ".sonata-gc"],
        &[".sonata-access.lock", ".sonata-store.lock"],
    )?;
    entries(&bytes.join("objects"), &["sha256"], &[])?;
    entries(&bytes.join("aliases"), &["sha1"], &[])?;
    let intents = bytes.join(".sonata-gc");
    if directory(&intents)? {
        for entry in fs::read_dir(intents).map_err(storage)? {
            let entry = entry.map_err(storage)?;
            let intent: DeleteIntent =
                serde_json::from_slice(&read_regular(&entry.path(), 4096).map_err(storage)?)
                    .map_err(storage)?;
            if intent.version != 1
                || !hash(&intent.sha256, 64)
                || !hash(&intent.sha1, 40)
                || entry.file_name() != format!("{}.json", intent.sha256).as_str()
            {
                return Err(storage("unknown Java archive deletion intent"));
            }
        }
    }
    let objects = bytes.join("objects/sha256");
    if directory(&objects)? {
        for shard in fs::read_dir(objects).map_err(storage)? {
            let shard = shard.map_err(storage)?;
            let prefix = shard
                .file_name()
                .into_string()
                .map_err(|_| storage("non-Unicode Java archive shard"))?;
            if !shard.file_type().map_err(storage)?.is_dir() || !hash(&prefix, 2) {
                return Err(storage("unknown Java archive shard"));
            }
            for entry in fs::read_dir(shard.path()).map_err(storage)? {
                let entry = entry.map_err(storage)?;
                let digest = entry
                    .file_name()
                    .into_string()
                    .map_err(|_| storage("non-Unicode Java archive object"))?;
                if !entry.file_type().map_err(storage)?.is_file()
                    || !hash(&digest, 64)
                    || !digest.starts_with(&prefix)
                {
                    return Err(storage("unknown Java archive object"));
                }
                entry
                    .metadata()
                    .map_err(storage)?
                    .modified()
                    .map_err(storage)?
                    .elapsed()
                    .map_err(storage)?;
            }
        }
    }
    let aliases = bytes.join("aliases/sha1");
    if directory(&aliases)? {
        for entry in fs::read_dir(aliases).map_err(storage)? {
            let entry = entry.map_err(storage)?;
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| storage("non-Unicode Java archive alias"))?;
            let body = read_regular(&entry.path(), 64).map_err(storage)?;
            if !hash(&name, 40) || !std::str::from_utf8(&body).is_ok_and(|body| hash(body, 64)) {
                return Err(storage("unknown Java archive alias"));
            }
        }
    }
    Ok(())
}

fn hash(value: &str, length: usize) -> bool {
    files::hash_valid(value, length) && value == value.to_ascii_lowercase()
}

fn entries(path: &Path, directories: &[&str], regular: &[&str]) -> Result<()> {
    if !directory(path)? {
        return Ok(());
    }
    for entry in fs::read_dir(path).map_err(storage)? {
        let entry = entry.map_err(storage)?;
        let kind = entry.file_type().map_err(storage)?;
        let name = entry.file_name();
        if !(kind.is_dir() && directories.iter().any(|known| name == *known)
            || kind.is_file() && regular.iter().any(|known| name == *known))
        {
            return Err(storage("unknown Java collection layout"));
        }
    }
    Ok(())
}

fn checkpoint(_root: &Path, _phase: &'static str) {
    #[cfg(test)]
    async_std::task::block_on(tests::checkpoint(_root, _phase));
}

#[cfg(test)]
mod tests;
