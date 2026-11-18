//! Private scratch directories, never runtime roots. A dead owner is required
//! for crash cleanup; names and age alone do not establish ownership.
use std::{fs, io, path::Path};

const OWNER: &str = ".sonata-owner";

pub(crate) struct OwnedDirectory(tempfile::TempDir);

impl OwnedDirectory {
    pub(crate) fn create(root: &Path, prefix: &str) -> io::Result<Self> {
        fs::create_dir_all(root)?;
        if !fs::symlink_metadata(root)?.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "scratch root is not a directory",
            ));
        }
        let directory = tempfile::Builder::new().prefix(prefix).tempdir_in(root)?;
        // Publish the complete PID atomically: a concurrent sweep must not see
        // a numeric prefix of a still-being-written PID as a different dead PID.
        // Exit before publication leaves an unknown directory, which is kept.
        crate::utils::process::write_owner(&directory.path().join(OWNER))?;
        Ok(Self(directory))
    }

    pub(crate) fn path(&self) -> &Path {
        self.0.path()
    }
}

/// Only direct children of a known scratch root are inspected. Symlink roots,
/// children and owner markers are not followed. Assumes stable ancestors, like
/// the other filesystem helpers; this is not a sandbox against external writers.
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
    let mut removed = 0;
    for entry in fs::read_dir(root)? {
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
        if !crate::utils::process::read_owner(&path.join(OWNER))
            .is_some_and(|pid| !process_alive(pid))
        {
            continue;
        }
        match fs::remove_dir_all(&path) {
            Ok(()) => removed += 1,
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
