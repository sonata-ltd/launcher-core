//! Replayable byte deletion, independent of domain roots and SQL metadata.
//! Admission and a complete domain snapshot are required before constructing
//! this capability; aliases are hints, never roots supplied by this module.
use super::{catalog::read_open_regular, ObjectStore, StoreMaintenance};
use crate::utils::files::{self, directory::Directory, identity::FileIdentity};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    ffi::OsStr,
    fs, io,
    path::Path,
    sync::Arc,
    time::Duration,
};

pub(crate) const INTENTS: &str = ".sonata-gc";

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
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
    directory: Arc<Directory>,
    #[cfg(test)]
    hook: fn(&Path, &'static str),
}

impl Collection {
    pub(crate) fn new(maintenance: Arc<StoreMaintenance>) -> io::Result<Self> {
        maintenance.directory.verify_location()?;
        let directory = maintenance.directory.clone();
        Ok(Self {
            maintenance,
            directory,
            #[cfg(test)]
            hook: |_, _| {},
        })
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
            let intents_directory = self.directory.open_relative(Path::new(INTENTS))?;
            for entry in fs::read_dir(&intents)? {
                let entry = entry?;
                let file = intents_directory.open_file(&entry.file_name())?;
                let intent: DeleteIntent = serde_json::from_slice(&read_open_regular(&file, 4096)?)
                    .map_err(io::Error::other)?;
                intent.validate()?;
                if entry.file_name() != format!("{}.json", intent.sha256).as_str() {
                    return Err(io::Error::other("deletion intent filename mismatch"));
                }
                if retained.retains(&intent) {
                    intents_directory.unlink(&entry.file_name())?;
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
                let relative = Path::new("objects/sha256").join(&prefix).join(&hash);
                let file = self.directory.open_entry_file(&relative)?;
                let digests = files::digest::retention_digests(&file)?;
                if digests.sha256 != hash || retained.sha1.contains(&digests.sha1) {
                    continue;
                }
                let intent = DeleteIntent {
                    version: 1,
                    sha256: hash.clone(),
                    sha1: digests.sha1,
                    size: digests.size_bytes,
                };
                let directory = self.directory.create_relative(Path::new(INTENTS))?;
                directory.write_bytes(
                    std::ffi::OsStr::new(&format!("{hash}.json")),
                    &serde_json::to_vec(&intent).map_err(io::Error::other)?,
                )?;
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
            let parent = path
                .parent()
                .ok_or_else(|| io::Error::other("object is missing its parent"))?;
            let directory = self
                .directory
                .open_relative(parent.strip_prefix(root).map_err(io::Error::other)?)?;
            match directory.remove_verified_file(OsStr::new(&intent.sha256), |file| {
                let digests = files::digest::retention_digests(file)?;
                if digests.sha256 != intent.sha256
                    || digests.sha1 != intent.sha1
                    || digests.size_bytes != intent.size
                {
                    return Err(io::Error::other("object changed since deletion intent"));
                }
                Ok(())
            }) {
                Ok(()) => {
                    self.checkpoint("unlink");
                    true
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => false,
                Err(error) => return Err(error),
            }
        };
        if regular_directory(&root.join("aliases"))?
            && regular_directory(&root.join("aliases/sha1"))?
        {
            let directory = self.directory.open_relative(Path::new("aliases/sha1"))?;
            let name = OsStr::new(&intent.sha1);
            match directory.open_file(name) {
                Ok(file) => {
                    if read_open_regular(&file, 64)? == intent.sha256.as_bytes() {
                        directory.verify_file(name, FileIdentity::from_handle(&file)?)?;
                        directory.unlink(name)?;
                    }
                }
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
        let directory = self.directory.open_relative(Path::new(INTENTS))?;
        for intent in completed {
            intent.validate()?;
            if !ObjectStore::new(self.maintenance.root().to_owned())
                .path_for(&intent.sha256)?
                .try_exists()?
            {
                let name = format!("{}.json", intent.sha256);
                directory.remove_verified_file(OsStr::new(&name), |file| {
                    let persisted: DeleteIntent =
                        serde_json::from_slice(&read_open_regular(file, 4096)?)
                            .map_err(io::Error::other)?;
                    if persisted != *intent {
                        return Err(io::Error::other("object deletion intent changed"));
                    }
                    Ok(())
                })?;
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
