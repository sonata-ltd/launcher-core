//! Private scratch directories, never runtime roots. A dead owner is required
//! for crash cleanup; names and age alone do not establish ownership.
use std::{
    ffi::OsStr,
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

use super::{directory::Directory, identity::FileIdentity};

const OWNER: &str = ".sonata-owner";

pub(crate) struct OwnedDirectory {
    path: PathBuf,
    parent: Arc<Directory>,
    directory: Option<Arc<Directory>>,
    retained: AtomicBool,
}

impl OwnedDirectory {
    #[cfg(test)]
    pub(crate) fn create(root: &Path, prefix: &str) -> io::Result<Self> {
        fs::create_dir_all(root)?;
        Self::create_in(&Directory::open(root)?, prefix)
    }

    pub(crate) fn create_in(root: &Arc<Directory>, prefix: &str) -> io::Result<Self> {
        let name = format!("{prefix}{}", uuid::Uuid::new_v4());
        let directory = root.create_private_child(OsStr::new(&name))?;
        let owned = Self {
            path: directory.path().to_owned(),
            parent: root.clone(),
            directory: Some(directory),
            retained: AtomicBool::new(false),
        };
        // Publish the complete PID atomically: a concurrent sweep must not see
        // a numeric prefix of a still-being-written PID as a different dead PID.
        // Exit before publication leaves an unknown directory, which is kept.
        let directory = owned.directory();
        directory.with_staged_file(
            |file| {
                file.write_all(std::process::id().to_string().as_bytes())?;
                file.sync_all()
            },
            |staged, ()| directory.publish(staged, directory, OsStr::new(OWNER)),
        )?;
        Ok(owned)
    }

    #[cfg(test)]
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn directory(&self) -> &Arc<Directory> {
        self.directory
            .as_ref()
            .expect("owned directory stays open until drop")
    }

    pub(crate) fn retain(&self) {
        self.retained.store(true, Ordering::Release);
    }
}

impl Drop for OwnedDirectory {
    fn drop(&mut self) {
        if self.retained.load(Ordering::Acquire) {
            return;
        }
        let Some(directory) = self.directory.take() else {
            return;
        };
        if let Err(error) = self.parent.remove_marked_tree(directory, OsStr::new(OWNER)) {
            tracing::debug!(path = %self.path.display(), %error, "could not remove scratch directory");
        }
    }
}

/// Only direct children of a known scratch root are inspected. Linked roots,
/// children and owner markers are excluded from ownership selection.
pub(crate) fn sweep(root: &Path, prefix: &str) -> io::Result<usize> {
    sweep_with(root, prefix, crate::utils::process::may_be_alive)
}

fn sweep_with(root: &Path, prefix: &str, process_alive: impl Fn(u32) -> bool) -> io::Result<usize> {
    match fs::symlink_metadata(root) {
        Ok(metadata) if metadata.is_dir() => {}
        Ok(_) => return Ok(0),
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(error),
    }
    let root = Directory::open(root)?;
    let mut removed = 0;
    for entry in fs::read_dir(root.path())? {
        let entry = entry?;
        if !entry.file_type()?.is_dir()
            || !entry
                .file_name()
                .as_encoded_bytes()
                .starts_with(prefix.as_bytes())
        {
            continue;
        }
        let path = entry.path();
        let result = (|| -> io::Result<bool> {
            let directory = root.open_relative(Path::new(&entry.file_name()))?;
            let marker = directory.open_file(OsStr::new(OWNER))?;
            if !crate::utils::process::read_owner_from(&marker)
                .is_some_and(|pid| !process_alive(pid))
            {
                return Ok(false);
            }
            directory.verify_file(OsStr::new(OWNER), FileIdentity::from_handle(&marker)?)?;
            drop(marker);
            root.remove_marked_tree(directory, OsStr::new(OWNER))?;
            Ok(true)
        })();
        match result {
            Ok(owned) => removed += usize::from(owned),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => {
                tracing::warn!(path = %path.display(), %error, "could not remove abandoned scratch directory")
            }
        }
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scratch_lifetime_and_unwind_remove_only_the_owned_directory() {
        let temp = tempfile::tempdir().unwrap();
        let previous = temp.path().join("scratch-previous");
        fs::create_dir(&previous).unwrap();
        let directory = OwnedDirectory::create(temp.path(), "scratch-").unwrap();
        let path = directory.path().to_owned();
        fs::write(path.join("data"), b"private bytes").unwrap();
        assert_eq!(sweep(temp.path(), "scratch-").unwrap(), 0);
        drop(directory);
        assert!(!path.exists());
        assert!(previous.exists());

        let mut path = None;
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let directory = OwnedDirectory::create(temp.path(), "scratch-").unwrap();
            path = Some(directory.path().to_owned());
            panic!("unwind while preparing scratch files");
        }));
        assert!(result.is_err());
        assert!(!path.unwrap().exists());
        assert!(previous.exists());
    }

    #[cfg(unix)]
    #[test]
    fn scratch_cleanup_preserves_a_replacement_directory() {
        let root = tempfile::tempdir().unwrap();
        let scratch = OwnedDirectory::create(root.path(), "scratch-").unwrap();
        let path = scratch.path().to_owned();
        let moved = root.path().join("moved");
        fs::rename(&path, &moved).unwrap();
        fs::create_dir(&path).unwrap();
        fs::write(path.join("keep"), b"user bytes").unwrap();

        drop(scratch);

        assert_eq!(fs::read(path.join("keep")).unwrap(), b"user bytes");
        assert!(moved.is_dir());
    }

    #[cfg(unix)]
    #[test]
    fn abandoned_scratch_cleanup_preserves_a_replacement() {
        let root = tempfile::tempdir().unwrap();
        let scratch = root.path().join("scratch-abandoned");
        let previous = root.path().join("previous");
        fs::create_dir(&scratch).unwrap();
        fs::write(scratch.join(OWNER), b"41").unwrap();
        fs::write(scratch.join("data"), b"abandoned output").unwrap();

        let removed = sweep_with(root.path(), "scratch-", |pid| {
            assert_eq!(pid, 41);
            fs::rename(&scratch, &previous).unwrap();
            fs::create_dir(&scratch).unwrap();
            fs::write(scratch.join("keep"), b"preserve these bytes").unwrap();
            false
        })
        .unwrap();

        assert_eq!(
            fs::read(scratch.join("keep")).unwrap(),
            b"preserve these bytes"
        );
        assert_eq!(
            fs::read(previous.join("data")).unwrap(),
            b"abandoned output"
        );
        assert_eq!(removed, 0);
    }

    #[test]
    fn scratch_sweep_requires_dead_ownership_and_a_known_namespace() {
        let temp = tempfile::tempdir().unwrap();
        for (name, owner) in [
            ("scratch-dead", Some("41")),
            ("scratch-live", Some("42")),
            ("scratch-reused-pid", Some("43")),
            ("scratch-legacy", None),
            ("scratch-incomplete", Some("")),
            ("scratch-invalid", Some("0")),
            ("scratch-oversized", Some("12345678901234567890")),
            ("unknown-dead", Some("41")),
        ] {
            let path = temp.path().join(name);
            fs::create_dir(&path).unwrap();
            if let Some(owner) = owner {
                fs::write(path.join(OWNER), owner).unwrap();
            }
            fs::write(path.join("data"), b"keep unless known dead").unwrap();
        }
        fs::write(temp.path().join("scratch-file"), b"not a directory").unwrap();
        assert_eq!(
            sweep_with(temp.path(), "scratch-", |pid| pid != 41).unwrap(),
            1
        );
        assert!(!temp.path().join("scratch-dead").exists());
        assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 8);
        assert_eq!(sweep_with(temp.path(), "scratch-", |_| true).unwrap(), 0);
        assert_eq!(sweep(&temp.path().join("absent"), "scratch-").unwrap(), 0);
    }

    #[cfg(unix)]
    #[test]
    fn scratch_sweep_does_not_follow_roots_children_or_owner_links() {
        use std::os::unix::fs::symlink;
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("root");
        let outside = temp.path().join("outside");
        fs::create_dir(&root).unwrap();
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join(OWNER), b"41").unwrap();
        fs::write(outside.join("data"), b"outside bytes").unwrap();
        symlink(&outside, root.join("scratch-link")).unwrap();
        let linked_owner = root.join("scratch-owner-link");
        fs::create_dir(&linked_owner).unwrap();
        symlink(outside.join(OWNER), linked_owner.join(OWNER)).unwrap();
        symlink(&root, temp.path().join("alias")).unwrap();
        assert_eq!(sweep_with(&root, "scratch-", |_| false).unwrap(), 0);
        assert_eq!(
            sweep_with(&temp.path().join("alias"), "scratch-", |_| false).unwrap(),
            0
        );
        assert!(OwnedDirectory::create(&temp.path().join("alias"), "scratch-").is_err());
        assert_eq!(fs::read(outside.join("data")).unwrap(), b"outside bytes");
    }
}
