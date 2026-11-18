//! Neutral immutable byte store. Roles, placements and provider provenance live
//! in domain repositories. Digest aliases are lookup hints, not semantic IDs.
use std::{
    fs, io,
    path::{Path, PathBuf},
};

use crate::utils::{
    cancellation::Cancellation,
    files::digest::{copy_and_digest, digest_file, FileDigests},
};

pub struct ObjectStore {
    root: PathBuf,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn store_lock_and_preparation_key_follow_the_actual_root_through_aliases() {
        let temp = tempfile::tempdir().unwrap();
        let actual = temp.path().join("actual");
        let first = ObjectStore::new(actual.clone());
        let lock = first.lock_exclusive().unwrap();
        let alias = temp.path().join("alias");
        std::os::unix::fs::symlink(&actual, &alias).unwrap();
        let second = ObjectStore::new(alias);
        assert_eq!(first.root_key().unwrap(), second.root_key().unwrap());
        assert!(second.lock_exclusive().is_err());
        drop(lock);
        assert!(second.lock_exclusive().is_ok());
    }

    #[test]
    fn aliases_survive_root_relocation_and_never_substitute_unverified_bytes() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("library.jar");
        fs::write(&source, b"same bytes in several roles").unwrap();
        let old = temp.path().join("old");
        let store = ObjectStore::new(old.clone());
        let cancel = Cancellation::new();
        let object = store.import_file(&source, None, &cancel).unwrap();
        let moved = temp.path().join("moved");
        fs::rename(old, &moved).unwrap();
        let store = ObjectStore::new(moved);
        let found = store
            .find_sha1(&object.digests.sha1, &cancel)
            .unwrap()
            .unwrap();
        assert_eq!(found.digests, object.digests);
        assert_eq!(found.path, store.path_for(&object.digests.sha256).unwrap());
        fs::write(&found.path, b"corrupt").unwrap();
        assert!(store
            .find_sha1(&object.digests.sha1, &cancel)
            .unwrap()
            .is_none());
        fs::write(
            store.alias_path(&object.digests.sha1).unwrap(),
            "../../outside",
        )
        .unwrap();
        assert!(store
            .find_sha1(&object.digests.sha1, &cancel)
            .unwrap()
            .is_none());
        assert!(store.find_sha1("../invalid", &cancel).is_err());
    }
}

#[derive(Debug)]
pub struct StoredObject {
    pub digests: FileDigests,
    pub path: PathBuf,
}

impl ObjectStore {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub(crate) fn root_key(&self) -> io::Result<Vec<u8>> {
        fs::create_dir_all(&self.root)?;
        Ok(crate::utils::files::path_key(&fs::canonicalize(
            &self.root,
        )?))
    }

    /// Content ownership/cleanup locks the actual store, even if two launcher
    /// roots reach it through different aliases. Runtime blob retention does
    /// not yet use this lock and must not run concurrent collection.
    pub(crate) fn lock_exclusive(&self) -> io::Result<fs::File> {
        fs::create_dir_all(&self.root)?;
        let lock = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(self.root.join(".sonata-store.lock"))?;
        lock.try_lock().map_err(io::Error::other)?;
        Ok(lock)
    }

    pub fn path_for(&self, sha256: &str) -> io::Result<PathBuf> {
        if sha256.len() != 64 || !sha256.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid SHA-256",
            ));
        }

        let hash = sha256.to_ascii_lowercase();
        Ok(self.root.join("objects/sha256").join(&hash[..2]).join(hash))
    }

    pub fn import_file(
        &self,
        source: &Path,
        expected_sha256: Option<&str>,
        cancellation: &Cancellation,
    ) -> io::Result<StoredObject> {
        self.import_checked(source, cancellation, |digests| {
            if expected_sha256
                .is_some_and(|expected| !expected.eq_ignore_ascii_case(&digests.sha256))
            {
                return Err(io::Error::other("source differs from preview"));
            }
            Ok(())
        })
    }

    pub(crate) fn import_checked<E: From<io::Error>>(
        &self,
        source: &Path,
        cancellation: &Cancellation,
        validate: impl FnOnce(&FileDigests) -> Result<(), E>,
    ) -> Result<StoredObject, E> {
        let staging = self.root.join("staging");
        fs::create_dir_all(&staging)?;
        let mut temporary = tempfile::NamedTempFile::new_in(&staging)?;
        let digests = copy_and_digest(source, &mut temporary, cancellation)?;
        validate(&digests)?;
        temporary.as_file().sync_all()?;

        let destination = self.path_for(&digests.sha256)?;
        let parent = destination
            .parent()
            .ok_or_else(|| io::Error::other("missing parent"))?;

        fs::create_dir_all(parent)?;
        cancellation.check().map_err(io::Error::from)?;

        match temporary.persist_noclobber(&destination) {
            Ok(_) => {}
            Err(e) if e.error.kind() == io::ErrorKind::AlreadyExists => {
                let (existing_digests, _) = digest_file(&destination, cancellation, |_| {})?;

                if existing_digests != digests {
                    return Err(io::Error::other("existing CAS object is corrupt").into());
                }
            }
            Err(e) => return Err(e.error.into()),
        }

        crate::utils::files::atomic_write(
            &self.alias_path(&digests.sha1)?,
            digests.sha256.as_bytes(),
        )
        .map_err(io::Error::other)?;
        Ok(StoredObject {
            digests,
            path: destination,
        })
    }

    fn alias_path(&self, sha1: &str) -> io::Result<PathBuf> {
        if !crate::utils::files::hash_valid(sha1, 40) {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "invalid SHA-1"));
        }
        Ok(self
            .root
            .join("aliases/sha1")
            .join(sha1.to_ascii_lowercase()))
    }

    /// Always verifies bytes: an alias (or a database row) is not a cache hit.
    pub(crate) fn find_sha1(
        &self,
        sha1: &str,
        cancel: &Cancellation,
    ) -> io::Result<Option<StoredObject>> {
        cancel.check().map_err(io::Error::from)?;
        let alias = self.alias_path(sha1)?;
        let sha256 = match fs::read_to_string(alias) {
            Ok(hash) => hash,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        let path = match self.path_for(&sha256) {
            Ok(path) => path,
            Err(_) => return Ok(None),
        };
        let (digests, _) = match digest_file(&path, cancel, |_| {}) {
            Ok(hashes) => hashes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        Ok((digests.sha256.eq_ignore_ascii_case(&sha256)
            && digests.sha1.eq_ignore_ascii_case(sha1))
        .then_some(StoredObject { digests, path }))
    }
}
