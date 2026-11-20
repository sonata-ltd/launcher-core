use std::{
    ffi::{OsStr, OsString},
    io::{self, ErrorKind, Write},
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use async_std::{fs, stream::StreamExt};

use crate::utils::files::{directory::Directory, identity::FileIdentity};

const OWNER_EXTENSION: &str = "sonata-owner";

pub(super) struct OwnedEntry {
    directory: Arc<Directory>,
    name: OsString,
    witness: std::fs::File,
}

impl OwnedEntry {
    pub(super) fn new(directory: Arc<Directory>, name: OsString, witness: std::fs::File) -> Self {
        Self {
            directory,
            name,
            witness,
        }
    }

    pub(super) fn publish(&self, name: &OsStr) -> io::Result<()> {
        self.verify()?;
        self.directory.require_regular_or_missing(name)?;
        self.directory.replace(&self.name, name)
    }

    fn verify(&self) -> io::Result<()> {
        self.directory
            .verify_file(&self.name, FileIdentity::from_handle(&self.witness)?)
    }
}

impl Drop for OwnedEntry {
    fn drop(&mut self) {
        if self.verify().is_ok() {
            let _ = self.directory.unlink(&self.name);
        }
    }
}

/// The marker is created before the partial file, and removed on normal return
/// or unwind. A process exit leaves enough ownership evidence for retention.
pub(crate) struct PartialOwner {
    _marker: OwnedEntry,
}

impl PartialOwner {
    #[cfg(test)]
    pub(crate) fn create(partial: &Path) -> std::io::Result<Self> {
        let parent = partial.parent().ok_or(ErrorKind::InvalidInput)?;
        let name = partial.file_name().ok_or(ErrorKind::InvalidInput)?;
        Self::create_in(Directory::open(parent)?, name)
    }

    pub(super) fn create_in(directory: Arc<Directory>, partial: &OsStr) -> io::Result<Self> {
        let name = owner_path(Path::new(partial)).into_os_string();
        let file = directory.with_staged_file(
            |file| {
                file.write_all(std::process::id().to_string().as_bytes())?;
                file.sync_all()?;
                file.try_clone()
            },
            |staged, file| {
                directory.publish(staged, &directory, &name)?;
                Ok::<_, io::Error>(file)
            },
        )?;
        Ok(Self {
            _marker: OwnedEntry::new(directory, name, file),
        })
    }
}

fn owner_path(partial: &Path) -> PathBuf {
    let mut path = partial.as_os_str().to_os_string();
    path.push(format!(".{OWNER_EXTENSION}"));
    path.into()
}

/// Retains unowned/legacy partials and every file with a live or unknown owner.
/// Age is an additional retention threshold, never evidence of abandonment.
pub async fn sweep_partials(root: &Path, min_age: Duration) -> usize {
    sweep_partials_with(root, min_age, crate::utils::process::may_be_alive).await
}

pub(super) async fn sweep_partials_with(
    root: &Path,
    min_age: Duration,
    process_alive: impl Fn(u32) -> bool,
) -> usize {
    let mut removed = 0usize;
    let mut queue: Vec<PathBuf> = vec![root.to_path_buf()];

    while let Some(dir) = queue.pop() {
        let mut entries = match fs::read_dir(&dir).await {
            Ok(entries) => entries,
            Err(e) if e.kind() == ErrorKind::NotFound => continue,
            Err(e) => {
                tracing::warn!(path = %dir.display(), error = %e, "sweep: cannot read directory");
                continue;
            }
        };

        while let Some(entry) = entries.next().await {
            let entry = match entry {
                Ok(entry) => entry,
                Err(e) => {
                    tracing::warn!(path = %dir.display(), error = %e, "sweep: unreadable entry");
                    continue;
                }
            };

            let path: PathBuf = entry.path().into();

            let file_type = match entry.file_type().await {
                Ok(ft) => ft,
                Err(e) => {
                    tracing::warn!(path = %path.display(), error = %e, "sweep: cannot stat");
                    continue;
                }
            };

            if file_type.is_dir() {
                queue.push(path);
                continue;
            }

            if !file_type.is_file() {
                continue;
            }

            let orphan_marker = path.extension() == Some(OsStr::new(OWNER_EXTENSION))
                && path
                    .file_stem()
                    .is_some_and(|stem| is_partial(Path::new(stem)));
            if !is_partial(&path) && !orphan_marker {
                continue;
            }
            let marker = if orphan_marker {
                path.clone()
            } else {
                owner_path(&path)
            };
            let owner =
                async_std::task::spawn_blocking(move || crate::utils::process::read_owner(&marker))
                    .await;
            if !owner.is_some_and(|pid| !process_alive(pid)) {
                continue;
            }
            if orphan_marker {
                let partial = path.with_file_name(path.file_stem().unwrap());
                // If the partial still exists, its branch owns removal/order.
                if !matches!(fs::symlink_metadata(partial).await, Err(e) if e.kind() == ErrorKind::NotFound)
                {
                    continue;
                }
            }

            let modified = match entry.metadata().await.and_then(|m| m.modified()) {
                Ok(modified) => modified,
                Err(e) => {
                    tracing::warn!(path = %path.display(), error = %e, "sweep: no mtime");
                    continue;
                }
            };

            match modified.elapsed() {
                Ok(age) if age >= min_age => {}
                _ => continue,
            }

            match fs::remove_file(&path).await {
                Ok(()) => {
                    tracing::debug!(path = %path.display(), "sweep: removed partial download");
                    if !orphan_marker {
                        removed += 1;
                        if let Err(error) = fs::remove_file(owner_path(&path)).await {
                            if error.kind() != ErrorKind::NotFound {
                                tracing::warn!(path = %path.display(), %error, "sweep: cannot remove owner marker");
                            }
                        }
                    }
                }
                Err(e) if e.kind() == ErrorKind::NotFound => {}
                Err(e) => {
                    tracing::warn!(path = %path.display(), error = %e, "sweep: cannot remove")
                }
            }
        }
    }

    removed
}

pub fn is_partial(path: &Path) -> bool {
    if path.extension() != Some(OsStr::new("part")) {
        return false;
    }

    let Some(name) = path.file_name() else {
        return false;
    };

    if !name.as_encoded_bytes().starts_with(b".") {
        return false;
    }

    let Some(stem) = path.file_stem() else {
        return false;
    };

    Path::new(stem)
        .extension()
        .and_then(OsStr::to_str)
        .is_some_and(|id| id.len() == 32 && id.bytes().all(|b| b.is_ascii_hexdigit()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn partial(root: &Path, name: &str) -> PathBuf {
        root.join(format!(".{name}.0123456789abcdef0123456789abcdef.part"))
    }

    #[test]
    fn partial_crash_writer() {
        let Some(root) = std::env::var_os("SONATA_TEST_PARTIAL_CRASH_ROOT") else {
            return;
        };
        let path = partial(Path::new(&root), "crashed.jar");
        let _owner = PartialOwner::create(&path).unwrap();
        std::fs::write(path, b"interrupted download").unwrap();
        std::process::exit(73);
    }

    #[async_std::test]
    async fn partial_owner_survives_process_exit_and_allows_startup_cleanup() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().to_owned();
        let output = async_std::task::spawn_blocking(move || {
            std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "utils::download::sweep::tests::partial_crash_writer",
                    "--nocapture",
                ])
                .env("SONATA_TEST_PARTIAL_CRASH_ROOT", root)
                .output()
                .unwrap()
        })
        .await;
        assert_eq!(
            output.status.code(),
            Some(73),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let path = partial(temp.path(), "crashed.jar");
        assert!(path.exists());
        assert!(owner_path(&path).exists());
        assert_eq!(sweep_partials(temp.path(), Duration::ZERO).await, 1);
        assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 0);
    }

    #[async_std::test]
    async fn live_downloader_ownership_survives_a_zero_age_sweep() {
        let temp = tempfile::tempdir().unwrap();
        let path = partial(temp.path(), "active.jar");
        let owner = PartialOwner::create(&path).unwrap();
        std::fs::write(&path, b"still downloading").unwrap();
        assert_eq!(sweep_partials(temp.path(), Duration::ZERO).await, 0);
        assert_eq!(std::fs::read(&path).unwrap(), b"still downloading");
        assert!(owner_path(&path).exists());
        drop(owner);
        assert!(!owner_path(&path).exists());
        // Absence of evidence is not evidence of a dead downloader.
        assert_eq!(sweep_partials(temp.path(), Duration::ZERO).await, 0);
        assert!(path.exists());
    }

    #[async_std::test]
    async fn retention_requires_a_confirmed_dead_owner_not_just_age_or_name() {
        let temp = tempfile::tempdir().unwrap();
        for (name, owner) in [
            ("dead", Some("41")),
            ("live", Some("42")),
            ("invalid", Some("bad")),
            ("zero", Some("0")),
            ("legacy", None),
        ] {
            let path = partial(temp.path(), name);
            std::fs::write(&path, name).unwrap();
            if let Some(owner) = owner {
                std::fs::write(owner_path(&path), owner).unwrap();
            }
        }
        assert_eq!(
            sweep_partials_with(temp.path(), Duration::ZERO, |pid| pid == 42).await,
            1
        );
        assert!(!partial(temp.path(), "dead").exists());
        assert!(!owner_path(&partial(temp.path(), "dead")).exists());
        for name in ["live", "invalid", "zero", "legacy"] {
            assert!(partial(temp.path(), name).exists());
        }
        let fresh = partial(temp.path(), "fresh");
        std::fs::write(&fresh, b"bytes").unwrap();
        std::fs::write(owner_path(&fresh), b"41").unwrap();
        assert_eq!(
            sweep_partials_with(temp.path(), Duration::from_secs(3600), |_| false).await,
            0
        );
        assert!(fresh.exists());
    }

    #[async_std::test]
    async fn orphan_markers_are_removed_only_after_their_owner_is_gone() {
        let temp = tempfile::tempdir().unwrap();
        let dead = owner_path(&partial(temp.path(), "dead"));
        let live = owner_path(&partial(temp.path(), "live"));
        std::fs::write(&dead, b"41").unwrap();
        std::fs::write(&live, b"42").unwrap();
        assert_eq!(
            sweep_partials_with(temp.path(), Duration::ZERO, |pid| pid == 42).await,
            0
        );
        assert!(!dead.exists());
        assert!(live.exists());
    }

    #[test]
    fn ownership_collision_preserves_an_existing_marker() {
        let temp = tempfile::tempdir().unwrap();
        let path = partial(temp.path(), "collision");
        let owner = PartialOwner::create(&path).unwrap();
        assert!(PartialOwner::create(&path).is_err());
        assert!(owner_path(&path).exists());
        drop(owner);
    }

    #[cfg(unix)]
    #[async_std::test]
    async fn retention_never_follows_partial_owner_or_directory_symlinks() {
        let temp = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let path = partial(temp.path(), "linked-owner");
        std::fs::write(&path, b"keep").unwrap();
        let marker = outside.path().join("marker");
        std::fs::write(&marker, b"41").unwrap();
        std::os::unix::fs::symlink(&marker, owner_path(&path)).unwrap();
        let outside_partial = partial(outside.path(), "outside");
        std::fs::write(&outside_partial, b"keep outside").unwrap();
        std::fs::write(owner_path(&outside_partial), b"41").unwrap();
        std::os::unix::fs::symlink(outside.path(), temp.path().join("external-directory")).unwrap();
        std::os::unix::fs::symlink(&outside_partial, partial(temp.path(), "linked-partial"))
            .unwrap();
        assert_eq!(
            sweep_partials_with(temp.path(), Duration::ZERO, |_| false).await,
            0
        );
        assert!(path.exists());
        assert!(outside_partial.exists());
    }
}
