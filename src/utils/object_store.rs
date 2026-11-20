//! Neutral immutable byte store. Roles, placements and provider provenance live
//! in domain repositories. Digest aliases are lookup hints, not semantic IDs.
use std::{
    ffi::OsStr,
    fs, io,
    io::Write,
    path::{Path, PathBuf},
};

use crate::utils::{
    cancellation::Cancellation,
    files::{
        digest::{copy_and_digest, digest_file, FileDigests},
        owned_directory::{self, OwnedDirectory},
    },
};

const IMPORT_PREFIX: &str = "import-";

mod access;
pub(crate) mod catalog;
pub(crate) mod collection;
pub(crate) use access::{StoreAccess, StoreLock, StoreMaintenance};

pub(crate) struct ObjectStore {
    root: PathBuf,
    /// Configured location; an alias here is followed once when opening.
    anchor: PathBuf,
    /// Launcher-owned subpath below the anchor, opened without following links.
    beneath: PathBuf,
}

impl ObjectStore {
    pub(crate) fn root(&self) -> &Path {
        &self.root
    }

    /// A store at a launcher-owned subpath of a configured root. The configured
    /// root may be an alias; no component of `relative` may redirect.
    pub(crate) fn beneath(anchor: &Path, relative: &Path) -> io::Result<Self> {
        crate::utils::files::relative(relative).map_err(io::Error::other)?;
        Ok(Self {
            root: anchor.join(relative),
            anchor: anchor.to_owned(),
            beneath: relative.to_owned(),
        })
    }

    /// Opens (creating when asked) the store root through the anchor. Returns
    /// the resolved root path and its held directory.
    fn open_root(
        &self,
        create: bool,
    ) -> io::Result<
        Option<(
            PathBuf,
            std::sync::Arc<crate::utils::files::directory::Directory>,
        )>,
    > {
        let directory = crate::utils::files::directory::Directory::open_beneath(
            &self.anchor,
            &self.beneath,
            create,
        )?;
        Ok(directory.map(|directory| (directory.path().to_owned(), directory)))
    }

    /// A launcher-owned store directory directly below its configured parent.
    pub(crate) fn child_of(path: &Path) -> io::Result<Self> {
        match (path.parent(), path.file_name()) {
            (Some(parent), Some(name)) => Self::beneath(parent, Path::new(name)),
            _ => Err(io::ErrorKind::InvalidInput.into()),
        }
    }

    fn create_root(
        &self,
    ) -> io::Result<(
        PathBuf,
        std::sync::Arc<crate::utils::files::directory::Directory>,
    )> {
        self.open_root(true)?
            .ok_or_else(|| io::ErrorKind::NotFound.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejected_and_cancelled_imports_remove_their_owned_staging() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        fs::write(&source, b"bytes").unwrap();
        let store = ObjectStore::new(temp.path().join("store"));
        assert!(store
            .import_file(&source, Some(&"0".repeat(64)), &Cancellation::new())
            .is_err());
        let cancel = Cancellation::new();
        cancel.cancel();
        assert!(store.import_file(&source, None, &cancel).is_err());
        assert_eq!(fs::read_dir(store.root.join("staging")).unwrap().count(), 0);
        assert!(!store.root.join("objects").exists());
    }

    const CRASH_ROOT: &str = "SONATA_TEST_IMPORT_CRASH_ROOT";

    #[test]
    fn import_crash_writer() {
        let Some(root) = std::env::var_os(CRASH_ROOT).map(PathBuf::from) else {
            return;
        };
        let store = ObjectStore::new(root.join("store"));
        let _: io::Result<StoredObject> =
            store.import_checked(&root.join("source"), &Cancellation::new(), |_| {
                std::process::exit(73)
            });
        panic!("missed import crash checkpoint");
    }

    #[test]
    fn process_exit_allows_import_staging_cleanup_without_collecting_bytes_or_live_owners() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        fs::write(&source, b"published bytes").unwrap();
        let store = ObjectStore::new(temp.path().join("store"));
        let object = store
            .import_file(&source, None, &Cancellation::new())
            .unwrap();
        let staging = store.root.join("staging");
        let live = OwnedDirectory::create(&staging, IMPORT_PREFIX).unwrap();
        fs::write(live.path().join("data"), b"active import").unwrap();
        let legacy = staging.join("legacy-unmarked-file");
        fs::write(&legacy, b"unknown ownership").unwrap();
        fs::write(&source, b"incomplete import").unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "utils::object_store::tests::import_crash_writer",
                "--nocapture",
            ])
            .env(CRASH_ROOT, temp.path())
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(73), "{output:?}");
        assert_eq!(fs::read_dir(&staging).unwrap().count(), 3);
        assert_eq!(store.sweep_staging().unwrap(), 1);
        assert_eq!(store.sweep_staging().unwrap(), 0);
        assert_eq!(fs::read_dir(&staging).unwrap().count(), 2);
        assert_eq!(
            fs::read(live.path().join("data")).unwrap(),
            b"active import"
        );
        assert_eq!(fs::read(legacy).unwrap(), b"unknown ownership");
        assert_eq!(fs::read(&object.path).unwrap(), b"published bytes");
        assert!(store
            .find_sha1(&object.digests.sha1, &Cancellation::new())
            .unwrap()
            .is_some());
    }

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

    #[cfg(unix)]
    fn redirected_store(temp: &Path) -> (ObjectStore, PathBuf) {
        let actual = temp.join("actual");
        fs::create_dir_all(actual.join("owned")).unwrap();
        let configured = temp.join("configured");
        std::os::unix::fs::symlink(&actual, &configured).unwrap();
        let store = ObjectStore::beneath(&configured, Path::new("owned/store")).unwrap();
        // A configured root alias is followed.
        drop(store.try_access().unwrap());
        // A launcher-owned component replaced after the store was resolved.
        let elsewhere = temp.join("elsewhere");
        fs::create_dir_all(elsewhere.join("store/staging/import-1")).unwrap();
        fs::write(elsewhere.join("store/staging/import-1/.sonata-owner"), b"1").unwrap();
        fs::rename(actual.join("owned"), temp.join("previous")).unwrap();
        std::os::unix::fs::symlink(&elsewhere, actual.join("owned")).unwrap();
        (store, elsewhere)
    }

    #[cfg(unix)]
    #[test]
    fn subpath_store_follows_a_configured_alias_but_not_a_redirected_owned_component() {
        let temp = tempfile::tempdir().unwrap();
        let (store, elsewhere) = redirected_store(temp.path());
        let before = tree(&elsewhere);
        assert!(store.try_access().is_err());
        assert!(store.try_maintenance().is_err());
        assert!(store.lock_exclusive().is_err());
        assert!(store.root_key().is_err());
        assert!(store.sweep_staging().is_err());
        assert_eq!(tree(&elsewhere), before);
    }

    #[cfg(unix)]
    fn tree(root: &Path) -> Vec<PathBuf> {
        let mut found = vec![root.to_owned()];
        if root.is_dir() {
            for entry in fs::read_dir(root).unwrap() {
                found.extend(tree(&entry.unwrap().path()));
            }
        }
        found.sort();
        found
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
/// A verified bytes snapshot, not a retention pin. A caller using its raw path
/// after this helper returns must retain operation admission (internal flows do)
/// or establish a durable root before a future collector may be enabled.
pub(crate) struct StoredObject {
    pub digests: FileDigests,
    pub path: PathBuf,
}

impl ObjectStore {
    pub fn new(root: PathBuf) -> Self {
        Self {
            anchor: root.clone(),
            root,
            beneath: PathBuf::new(),
        }
    }

    /// Scratch-only retention, safe during other live imports. This never
    /// collects published objects, aliases or legacy files and requires no SQL
    /// roots. Imports from an exited process cannot register or publish later.
    pub(crate) fn sweep_staging(&self) -> io::Result<usize> {
        let Some((_, root)) = self.open_root(false)? else {
            return Ok(0);
        };
        match root.open_optional(Path::new("staging"))? {
            Some(staging) => owned_directory::sweep_in(&staging, &[IMPORT_PREFIX]),
            None => Ok(0),
        }
    }

    pub(crate) fn root_key(&self) -> io::Result<Vec<u8>> {
        let (root, _) = self.create_root()?;
        Ok(crate::utils::files::path_key(&root))
    }

    /// Content service ownership, distinct from the shared admission/exclusive
    /// maintenance barrier. Both follow the actual store through aliases.
    pub(crate) fn lock_exclusive(&self) -> io::Result<StoreLock> {
        let (_, root) = self.create_root()?;
        let lock = root.open_lock(OsStr::new(".sonata-store.lock"))?;
        StoreLock::exclusive(lock)
    }

    /// Pure layout resolution: the returned path neither verifies nor pins bytes.
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

    /// Synchronous admission is fail-fast (`WouldBlock` during maintenance).
    /// The returned snapshot does not extend this call's admission lifetime.
    #[cfg(test)]
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

    #[cfg(test)]
    pub(crate) fn import_checked<E: From<io::Error>>(
        &self,
        source: &Path,
        cancellation: &Cancellation,
        validate: impl FnOnce(&FileDigests) -> Result<(), E>,
    ) -> Result<StoredObject, E> {
        cancellation.check().map_err(io::Error::from)?;
        self.try_access()?
            .import_checked(source, cancellation, validate)
    }
}

impl StoreAccess {
    pub(crate) fn import_checked<E: From<io::Error>>(
        &self,
        source: &Path,
        cancellation: &Cancellation,
        validate: impl FnOnce(&FileDigests) -> Result<(), E>,
    ) -> Result<StoredObject, E> {
        cancellation.check().map_err(io::Error::from)?;
        let root = &self.directory;
        let staging =
            OwnedDirectory::create_in(&root.create_child(OsStr::new("staging"))?, IMPORT_PREFIX)?;
        let staging = staging.directory();
        staging.with_staged_file(
            |temporary| {
                let digests = copy_and_digest(source, temporary, cancellation)?;
                validate(&digests)?;
                temporary.sync_all()?;
                Ok(digests)
            },
            |temporary, digests| {
                let path = self.path_for(&digests.sha256)?;
                let parent =
                    root.create_relative(&Path::new("objects/sha256").join(&digests.sha256[..2]))?;
                let aliases = root.create_relative(Path::new("aliases/sha1"))?;
                let name = OsStr::new(&digests.sha256);
                let destination = parent.path().join(name);
                cancellation.check().map_err(io::Error::from)?;
                match staging.publish(temporary, &parent, name) {
                    Ok(()) => (),
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                        parent.verify_location()?;
                        let (existing, _) = digest_file(&destination, cancellation, |_| {})?;
                        if existing != digests {
                            return Err(io::Error::other("existing CAS object is corrupt").into());
                        }
                    }
                    Err(error) => return Err(error.into()),
                }
                aliases.with_staged_file(
                    |file| {
                        file.write_all(digests.sha256.as_bytes())?;
                        file.sync_all()
                    },
                    |temporary, ()| aliases.replace(temporary, OsStr::new(&digests.sha1)),
                )?;
                Ok(StoredObject { digests, path })
            },
        )
    }
}

impl ObjectStore {
    fn alias_path(&self, sha1: &str) -> io::Result<PathBuf> {
        if !crate::utils::files::hash_valid(sha1, 40) {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "invalid SHA-1"));
        }
        Ok(self
            .root
            .join("aliases/sha1")
            .join(sha1.to_ascii_lowercase()))
    }

    /// Lookup by authoritative byte identity still requires a fresh verification.
    /// The returned snapshot does not retain this method's admission lifetime.
    pub(crate) fn find_sha256(
        &self,
        sha256: &str,
        cancel: &Cancellation,
    ) -> io::Result<Option<StoredObject>> {
        cancel.check().map_err(io::Error::from)?;
        let _access = self.try_access()?;
        let path = self.path_for(sha256)?;
        let relative = path.strip_prefix(&self.root).map_err(io::Error::other)?;
        let path = crate::utils::files::child(&self.root, relative).map_err(io::Error::other)?;
        let (digests, _) = match digest_file(&path, cancel, |_| {}) {
            Ok(hashes) => hashes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        Ok(digests
            .sha256
            .eq_ignore_ascii_case(sha256)
            .then_some(StoredObject { digests, path }))
    }

    /// Always verifies bytes: an alias (or a database row) is not a cache hit.
    pub(crate) fn find_sha1(
        &self,
        sha1: &str,
        cancel: &Cancellation,
    ) -> io::Result<Option<StoredObject>> {
        cancel.check().map_err(io::Error::from)?;
        let _access = self.try_access()?;
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
