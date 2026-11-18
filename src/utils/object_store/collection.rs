//! Replayable byte deletion, independent of domain roots and SQL metadata.
//! Admission and a complete domain snapshot are required before constructing
//! this capability; aliases are hints, never roots supplied by this module.
use super::{catalog::read_regular, ObjectStore, StoreMaintenance};
use crate::utils::{cancellation::Cancellation, files};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    fs, io,
    path::Path,
    sync::Arc,
    time::Duration,
};

pub(crate) const INTENTS: &str = ".sonata-gc";

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DeleteIntent {
    pub(crate) version: u32,
    pub(crate) sha256: String,
    pub(crate) sha1: String,
    pub(crate) size: u64,
}

impl DeleteIntent {
    fn validate(&self) -> io::Result<()> {
        if self.version != 1
            || !files::hash_valid(&self.sha256, 64)
            || !files::hash_valid(&self.sha1, 40)
            || self.sha256 != self.sha256.to_ascii_lowercase()
            || self.sha1 != self.sha1.to_ascii_lowercase()
        {
            return Err(io::Error::other("invalid object deletion intent"));
        }
        Ok(())
    }
}

#[derive(Default)]
pub(crate) struct Roots {
    pub(crate) sha256: HashSet<String>,
    /// Checksums from durable profiles/placements or verified retained bytes,
    /// not values read from digest/URL aliases.
    pub(crate) sha1: HashSet<String>,
}

impl Roots {
    fn retains(&self, intent: &DeleteIntent) -> bool {
        self.sha256.contains(&intent.sha256) || self.sha1.contains(&intent.sha1)
    }
}

#[derive(Clone)]
pub(crate) struct Collection {
    maintenance: Arc<StoreMaintenance>,
    #[cfg(test)]
    hook: fn(&Path, &'static str),
}

impl Collection {
    pub(crate) fn new(maintenance: Arc<StoreMaintenance>) -> Self {
        Self {
            maintenance,
            #[cfg(test)]
            hook: |_, _| {},
        }
    }

    #[cfg(test)]
    pub(crate) fn with_test_hook(mut self, hook: fn(&Path, &'static str)) -> Self {
        self.hook = hook;
        self
    }

    fn checkpoint(&self, _phase: &'static str) {
        #[cfg(test)]
        (self.hook)(self.maintenance.root(), _phase);
    }

    pub(crate) fn plan(&self, retained: &Roots, age: Duration) -> io::Result<Vec<DeleteIntent>> {
        let root = self.maintenance.root();
        let store = ObjectStore::new(root.to_owned());
        let intents = root.join(INTENTS);
        let mut plan = HashMap::new();
        if regular_directory(&intents)? {
            for entry in fs::read_dir(&intents)? {
                let entry = entry?;
                let intent: DeleteIntent =
                    serde_json::from_slice(&read_regular(&entry.path(), 4096)?)
                        .map_err(io::Error::other)?;
                intent.validate()?;
                if entry.file_name() != format!("{}.json", intent.sha256).as_str() {
                    return Err(io::Error::other("deletion intent filename mismatch"));
                }
                if retained.retains(&intent) {
                    fs::remove_file(entry.path())?;
                } else {
                    let target = store.path_for(&intent.sha256)?;
                    if regular_directory(&root.join("objects"))?
                        && regular_directory(&root.join("objects/sha256"))?
                        && regular_directory(target.parent().unwrap())?
                    {
                        match fs::symlink_metadata(&target) {
                            Ok(metadata) if metadata.is_file() => {
                                if metadata
                                    .modified()?
                                    .elapsed()
                                    .map_or(true, |elapsed| elapsed < age)
                                {
                                    continue;
                                }
                            }
                            Ok(_) => {
                                return Err(io::Error::other(
                                    "replayed deletion target is not regular",
                                ))
                            }
                            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                            Err(error) => return Err(error),
                        }
                    }
                    plan.insert(intent.sha256.clone(), intent);
                }
            }
        }
        let objects = root.join("objects/sha256");
        if !regular_directory(&root.join("objects"))? || !regular_directory(&objects)? {
            return Ok(plan.into_values().collect());
        }
        for shard in fs::read_dir(objects)? {
            let shard = shard?;
            if !shard.file_type()?.is_dir() {
                continue;
            }
            let Some(prefix) = shard.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            if prefix.len() != 2 || !files::hash_valid(&prefix, 2) {
                continue;
            }
            for entry in fs::read_dir(shard.path())? {
                let entry = entry?;
                let Some(hash) = entry.file_name().to_str().map(str::to_owned) else {
                    continue;
                };
                if !files::hash_valid(&hash, 64)
                    || !hash.starts_with(&prefix)
                    || hash != hash.to_ascii_lowercase()
                    || retained.sha256.contains(&hash)
                    || plan.contains_key(&hash)
                    || !entry.file_type()?.is_file()
                {
                    continue;
                }
                let metadata = entry.metadata()?;
                if metadata
                    .modified()?
                    .elapsed()
                    .map_or(true, |elapsed| elapsed < age)
                {
                    continue;
                }
                let (digests, _) = files::digest::digest_file(
                    &store.path_for(&hash)?,
                    &Cancellation::new(),
                    |_| {},
                )?;
                if digests.sha256 != hash || retained.sha1.contains(&digests.sha1) {
                    continue;
                }
                let intent = DeleteIntent {
                    version: 1,
                    sha256: hash.clone(),
                    sha1: digests.sha1,
                    size: digests.size_bytes,
                };
                fs::create_dir_all(&intents)?;
                files::atomic_write(
                    &intents.join(format!("{hash}.json")),
                    &serde_json::to_vec(&intent).map_err(io::Error::other)?,
                )
                .map_err(io::Error::other)?;
                self.checkpoint("intent");
                plan.insert(hash, intent);
            }
        }
        Ok(plan.into_values().collect())
    }

    pub(crate) fn unlink(&self, intent: &DeleteIntent) -> io::Result<bool> {
        intent.validate()?;
        let root = self.maintenance.root();
        let path = ObjectStore::new(root.to_owned()).path_for(&intent.sha256)?;
        let directories_present = regular_directory(&root.join("objects"))?
            && regular_directory(&root.join("objects/sha256"))?
            && regular_directory(path.parent().unwrap())?;
        let removed = if !directories_present {
            false
        } else {
            match fs::symlink_metadata(&path) {
                Ok(metadata) if metadata.is_file() => {
                    let (digests, _) =
                        files::digest::digest_file(&path, &Cancellation::new(), |_| {})?;
                    if digests.sha256 != intent.sha256
                        || digests.sha1 != intent.sha1
                        || digests.size_bytes != intent.size
                    {
                        return Err(io::Error::other("object changed since deletion intent"));
                    }
                    fs::remove_file(path)?;
                    self.checkpoint("unlink");
                    true
                }
                Ok(_) => return Err(io::Error::other("deletion target is not regular")),
                Err(error) if error.kind() == io::ErrorKind::NotFound => false,
                Err(error) => return Err(error),
            }
        };
        if regular_directory(&root.join("aliases"))?
            && regular_directory(&root.join("aliases/sha1"))?
        {
            let alias = root.join("aliases/sha1").join(&intent.sha1);
            match read_regular(&alias, 64) {
                Ok(body) if body == intent.sha256.as_bytes() => fs::remove_file(alias)?,
                Ok(_) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
        }
        self.checkpoint("alias");
        Ok(removed)
    }

    /// Called only after every domain metadata commit has completed. Removing
    /// bytes never removes identity/provenance or semantic placements here.
    pub(crate) fn clear_finished_intents(&self, completed: &[DeleteIntent]) -> io::Result<()> {
        let intents = self.maintenance.root().join(INTENTS);
        if !regular_directory(&intents)? {
            return Ok(());
        }
        for intent in completed {
            intent.validate()?;
            if !ObjectStore::new(self.maintenance.root().to_owned())
                .path_for(&intent.sha256)?
                .try_exists()?
            {
                fs::remove_file(intents.join(format!("{}.json", intent.sha256)))?;
            }
        }
        Ok(())
    }
}

fn regular_directory(path: &Path) -> io::Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() => Ok(true),
        Ok(_) => Err(io::Error::other(
            "retention directory is a link or special file",
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}
