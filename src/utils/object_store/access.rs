//! Cross-process admission for CAS readers/importers and exclusive maintenance.
//! This lock is distinct from the content service's lifetime ownership lock.
use std::{fs, io, ops::Deref, sync::Arc, time::Duration};

use super::ObjectStore;
use crate::utils::cancellation::Cancellation;

const LOCK: &str = ".sonata-access.lock";

struct StoreLock(fs::File);

impl Drop for StoreLock {
    fn drop(&mut self) {
        // A concurrent fork can briefly inherit the open file description. Just
        // closing our descriptor would let that child retain a Unix flock until
        // exec/exit; explicitly unlock when the last logical owner finishes.
        if let Err(error) = self.0.unlock() {
            tracing::warn!(%error, "could not release CAS admission lock");
        }
    }
}

#[derive(Clone)]
pub(crate) struct StoreAccess {
    store: Arc<ObjectStore>,
    _lock: Arc<StoreLock>,
}

impl Deref for StoreAccess {
    type Target = ObjectStore;

    fn deref(&self) -> &ObjectStore {
        &self.store
    }
}

pub(crate) struct StoreMaintenance {
    root: std::path::PathBuf,
    root_key: Vec<u8>,
    _lock: StoreLock,
}

impl StoreMaintenance {
    pub(crate) fn root(&self) -> &std::path::Path {
        &self.root
    }
    pub(crate) fn root_key(&self) -> &[u8] {
        &self.root_key
    }
}

impl ObjectStore {
    fn access_file(&self) -> io::Result<(ObjectStore, fs::File)> {
        fs::create_dir_all(&self.root)?;
        let root = fs::canonicalize(&self.root)?;
        let path = root.join(LOCK);
        match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.is_file() => {}
            Ok(_) => return Err(io::Error::other("CAS access lock is not a regular file")),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        let file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)?;
        Ok((ObjectStore::new(root), file))
    }

    pub(crate) fn try_access(&self) -> io::Result<StoreAccess> {
        let (store, file) = self.access_file()?;
        file.try_lock_shared().map_err(lock_error)?;
        Ok(StoreAccess {
            store: Arc::new(store),
            _lock: Arc::new(StoreLock(file)),
        })
    }

    /// Cancellation-aware admission; never block the executor on an OS lock.
    /// Clones may follow blocking workers so dropping a caller's future cannot
    /// release admission while the worker still reads or publishes CAS bytes.
    pub(crate) async fn access(&self, cancel: &Cancellation) -> io::Result<StoreAccess> {
        loop {
            cancel.check()?;
            match self.try_access() {
                Ok(access) => return Ok(access),
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    cancel
                        .run(async_std::task::sleep(Duration::from_millis(10)))
                        .await?;
                }
                Err(error) => return Err(error),
            }
        }
    }

    /// A capability for exclusive maintenance, not proof that bytes are garbage.
    /// A collector must still validate all durable preparations/catalog roots.
    pub(crate) fn try_maintenance(&self) -> io::Result<StoreMaintenance> {
        let (store, file) = self.access_file()?;
        file.try_lock().map_err(lock_error)?;
        Ok(StoreMaintenance {
            root: store.root.clone(),
            root_key: store.root_key()?,
            _lock: StoreLock(file),
        })
    }

    pub(crate) async fn maintenance(&self, cancel: &Cancellation) -> io::Result<StoreMaintenance> {
        loop {
            cancel.check()?;
            match self.try_maintenance() {
                Ok(maintenance) => return Ok(maintenance),
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    cancel
                        .run(async_std::task::sleep(Duration::from_millis(10)))
                        .await?;
                }
                Err(error) => return Err(error),
            }
        }
    }
}

fn lock_error(error: fs::TryLockError) -> io::Error {
    match error {
        fs::TryLockError::WouldBlock => io::ErrorKind::WouldBlock.into(),
        fs::TryLockError::Error(error) => error,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn independent_handles_share_admission_and_exclude_maintenance_until_the_last_clone() {
        let temp = tempfile::tempdir().unwrap();
        let store = ObjectStore::new(temp.path().join("store"));
        let first = store.try_access().unwrap();
        let second = ObjectStore::new(store.root.clone()).try_access().unwrap();
        let worker = first.clone();
        assert_eq!(
            store.try_maintenance().err().unwrap().kind(),
            io::ErrorKind::WouldBlock
        );
        drop(first);
        drop(second);
        assert!(store.try_maintenance().is_err());
        drop(worker);
        let maintenance = store.try_maintenance().unwrap();
        assert_eq!(maintenance.root_key(), store.root_key().unwrap());
        assert_eq!(
            store.try_access().err().unwrap().kind(),
            io::ErrorKind::WouldBlock
        );
        assert!(store.try_maintenance().is_err());
        drop(maintenance);
        assert!(store.try_access().is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn final_owner_unlocks_even_while_a_duplicated_descriptor_remains_open() {
        let temp = tempfile::tempdir().unwrap();
        let store = ObjectStore::new(temp.path().join("store"));
        let access = store.try_access().unwrap();
        let inherited = access._lock.0.try_clone().unwrap();
        drop(access);
        let maintenance = store.try_maintenance().unwrap();
        let inherited_maintenance = maintenance._lock.0.try_clone().unwrap();
        drop(maintenance);
        assert!(store.try_access().is_ok());
        drop(inherited);
        drop(inherited_maintenance);
    }

    #[cfg(unix)]
    #[test]
    fn admission_uses_the_actual_store_through_root_aliases() {
        let temp = tempfile::tempdir().unwrap();
        let store = ObjectStore::new(temp.path().join("actual"));
        let access = store.try_access().unwrap();
        let alias = temp.path().join("alias");
        std::os::unix::fs::symlink(&store.root, &alias).unwrap();
        let aliased = ObjectStore::new(alias);
        assert_eq!(access.root_key().unwrap(), aliased.root_key().unwrap());
        assert!(aliased.try_maintenance().is_err());
        drop(access);
        let _maintenance = aliased.try_maintenance().unwrap();
        assert!(store.try_access().is_err());
    }

    #[async_std::test]
    async fn waiting_for_admission_is_cancellable_without_reading_or_creating_staging() {
        let temp = tempfile::tempdir().unwrap();
        let store = ObjectStore::new(temp.path().join("store"));
        let maintenance = store.try_maintenance().unwrap();
        let cancel = Cancellation::new();
        let trigger = cancel.clone();
        let (result, ()) = futures::join!(store.access(&cancel), async {
            async_std::task::yield_now().await;
            trigger.cancel();
        });
        assert_eq!(result.err().unwrap().kind(), io::ErrorKind::Interrupted);
        assert!(!store.root.join("staging").exists());
        drop(maintenance);
        assert!(store.access(&Cancellation::new()).await.is_ok());
    }

    #[test]
    fn maintenance_blocks_low_level_imports_and_alias_readers_before_staging() {
        let temp = tempfile::tempdir().unwrap();
        let store = ObjectStore::new(temp.path().join("store"));
        let source = temp.path().join("source");
        fs::write(&source, b"bytes").unwrap();
        let _maintenance = store.try_maintenance().unwrap();
        let cancel = Cancellation::new();
        assert_eq!(
            store
                .import_file(&source, None, &cancel)
                .unwrap_err()
                .kind(),
            io::ErrorKind::WouldBlock
        );
        assert_eq!(
            store
                .find_sha1(&"0".repeat(40), &cancel)
                .unwrap_err()
                .kind(),
            io::ErrorKind::WouldBlock
        );
        assert!(!store.root.join("staging").exists());
        assert!(!store.root.join("objects").exists());
    }

    #[cfg(unix)]
    #[test]
    fn lock_symlinks_and_directories_are_not_followed_or_replaced() {
        let temp = tempfile::tempdir().unwrap();
        let store = ObjectStore::new(temp.path().join("store"));
        fs::create_dir_all(&store.root).unwrap();
        let outside = temp.path().join("outside");
        fs::write(&outside, b"user bytes").unwrap();
        let lock = store.root.join(LOCK);
        std::os::unix::fs::symlink(&outside, &lock).unwrap();
        assert!(store.try_access().is_err());
        assert!(store.try_maintenance().is_err());
        assert_eq!(fs::read(outside).unwrap(), b"user bytes");
        fs::remove_file(&lock).unwrap();
        fs::create_dir(&lock).unwrap();
        assert!(store.try_access().is_err());
        assert!(store.try_maintenance().is_err());
        assert!(lock.is_dir());
    }

    #[async_std::test]
    async fn dropping_the_caller_does_not_release_a_live_import_workers_admission() {
        use futures::poll;
        let temp = tempfile::tempdir().unwrap();
        let store = ObjectStore::new(temp.path().join("store"));
        let source = temp.path().join("source");
        fs::write(&source, b"worker bytes").unwrap();
        let access = store.try_access().unwrap();
        let (entered, entry) = async_channel::bounded(1);
        let (resume, resumed) = std::sync::mpsc::channel();
        let (done, finished) = async_channel::bounded(1);
        let mut caller = Box::pin(async move {
            async_std::task::spawn_blocking(move || {
                let result: io::Result<_> =
                    access.import_checked(&source, &Cancellation::new(), |_| {
                        entered.send_blocking(()).unwrap();
                        resumed.recv_timeout(Duration::from_secs(10)).unwrap();
                        Ok(())
                    });
                drop(access);
                done.send_blocking(result.map(|object| object.digests.sha256))
                    .unwrap();
            })
            .await;
        });
        assert!(poll!(caller.as_mut()).is_pending());
        async_std::future::timeout(Duration::from_secs(10), entry.recv())
            .await
            .unwrap()
            .unwrap();
        drop(caller);
        assert_eq!(
            store.try_maintenance().err().unwrap().kind(),
            io::ErrorKind::WouldBlock
        );
        resume.send(()).unwrap();
        let hash = async_std::future::timeout(Duration::from_secs(10), finished.recv())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(
            fs::read(store.path_for(&hash).unwrap()).unwrap(),
            b"worker bytes"
        );
        assert!(store.try_maintenance().is_ok());
    }

    const CHILD_ROOT: &str = "SONATA_TEST_CAS_ADMISSION_ROOT";
    const CHILD_MODE: &str = "SONATA_TEST_CAS_ADMISSION_MODE";

    #[test]
    fn admission_lock_holder() {
        use std::io::{Read, Write};
        let Some(root) = std::env::var_os(CHILD_ROOT) else {
            return;
        };
        let store = ObjectStore::new(root.into());
        let _access;
        let _maintenance;
        if std::env::var(CHILD_MODE).unwrap() == "shared" {
            _access = Some(store.try_access().unwrap());
            _maintenance = None;
        } else {
            _access = None;
            _maintenance = Some(store.try_maintenance().unwrap());
        }
        println!("CAS_ADMITTED");
        io::stdout().flush().unwrap();
        io::stdin().read_exact(&mut [0]).unwrap();
        std::process::exit(73);
    }

    #[test]
    fn independent_processes_exclude_maintenance_and_release_locks_after_process_exit() {
        use std::io::{BufRead, Write};
        for mode in ["shared", "exclusive"] {
            let temp = tempfile::tempdir().unwrap();
            let store = ObjectStore::new(temp.path().join("store"));
            let mut child = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "utils::object_store::access::tests::admission_lock_holder",
                    "--nocapture",
                ])
                .env(CHILD_ROOT, &store.root)
                .env(CHILD_MODE, mode)
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .spawn()
                .unwrap();
            let output = io::BufReader::new(child.stdout.take().unwrap());
            assert!(output
                .lines()
                .any(|line| line.unwrap().ends_with("CAS_ADMITTED")));
            assert_eq!(
                store.try_maintenance().err().unwrap().kind(),
                io::ErrorKind::WouldBlock
            );
            if mode == "shared" {
                assert!(store.try_access().is_ok());
            } else {
                assert_eq!(
                    store.try_access().err().unwrap().kind(),
                    io::ErrorKind::WouldBlock
                );
            }
            child.stdin.take().unwrap().write_all(&[1]).unwrap();
            assert_eq!(child.wait().unwrap().code(), Some(73));
            assert!(store.try_access().is_ok());
            assert!(store.try_maintenance().is_ok());
        }
    }
}
