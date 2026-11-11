use sha1::{Digest, Sha1};
use sha2::Sha256;
use std::{
    fs,
    io::{Read, Write},
    path::{Component, Path, PathBuf},
};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum FileError {
    #[error("invalid path: {0}")]
    InvalidPath(String),

    #[error(transparent)]
    Io(#[from] std::io::Error),
}

type Result<T> = std::result::Result<T, FileError>;

pub fn relative(path: &Path) -> Result<()> {
    if path.as_os_str().is_empty()
        || path
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
        || path.to_string_lossy().contains(':')
    {
        return Err(FileError::InvalidPath(format!(
            "unsafe relative path: {}",
            path.display()
        )));
    }

    Ok(())
}

pub fn child(root: &Path, path: &Path) -> Result<PathBuf> {
    relative(path)?;
    let full = root.join(path);
    if full.exists() && !full.canonicalize()?.starts_with(root.canonicalize()?) {
        return Err(FileError::InvalidPath(
            "artifact symlink escapes root".into(),
        ));
    }

    Ok(full)
}

pub fn sha1_file(path: &Path) -> std::io::Result<String> {
    let mut input = fs::File::open(path)?;
    let mut digest = Sha1::new();
    let mut buf = [0; 65536];

    loop {
        let n = input.read(&mut buf)?;

        if n == 0 {
            break;
        }

        digest.update(&buf[..n]);
    }

    Ok(format!("{:x}", digest.finalize()))
}

pub fn sha256_file(path: &Path) -> std::io::Result<String> {
    let mut input = fs::File::open(path)?;
    let mut digest = Sha256::new();
    let mut buf = [0; 65536];

    loop {
        let n = input.read(&mut buf)?;

        if n == 0 {
            break;
        }

        digest.update(&buf[..n]);
    }

    Ok(format!("{:x}", digest.finalize()))
}

pub fn matches(path: &Path, sha1: &str) -> std::io::Result<bool> {
    match sha1_file(path) {
        Ok(actual) => Ok(actual.eq_ignore_ascii_case(sha1)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e),
    }
}

pub fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| FileError::InvalidPath("path has no parent".into()))?;

    fs::create_dir_all(parent)?;

    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    file.write_all(bytes)?;
    file.as_file().sync_all()?;
    file.persist(path).map_err(|e| FileError::Io(e.error))?;

    Ok(())
}

pub fn hash_valid(value: &str, len: usize) -> bool {
    value.len() == len && value.bytes().all(|b| b.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cache_checks_content_and_missing_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x");
        assert!(!matches(&path, "x").unwrap());
        atomic_write(&path, b"abc").unwrap();
        let hash = sha1_file(&path).unwrap();
        assert!(matches(&path, &hash).unwrap());
        atomic_write(&path, b"xyz").unwrap();
        assert!(!matches(&path, &hash).unwrap());
    }
    #[test]
    fn rejects_traversal() {
        for path in ["", ".", "..", "../x", "a/../x", "a:b"] {
            assert!(
                matches!(relative(Path::new(path)), Err(FileError::InvalidPath(_))),
                "accepted invalid path: {path:?}"
            );
        }
        let dir = tempfile::tempdir().unwrap();
        assert!(matches!(
            relative(dir.path()),
            Err(FileError::InvalidPath(_))
        ));
    }

    #[test]
    fn child_accepts_existing_and_not_yet_created_files_inside_root() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("existing.jar"), b"runtime").unwrap();

        for path in ["existing.jar", "org/example/library.jar"] {
            assert_eq!(
                child(dir.path(), Path::new(path)).unwrap(),
                dir.path().join(path)
            );
        }
        assert!(!dir.path().join("org").exists());
    }

    #[test]
    fn child_rejects_traversal_even_when_target_exists() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("root");
        fs::create_dir(&root).unwrap();
        fs::write(dir.path().join("outside.jar"), b"outside").unwrap();

        assert!(matches!(
            child(&root, Path::new("../outside.jar")),
            Err(FileError::InvalidPath(_))
        ));
    }

    #[test]
    fn atomic_write_rejects_target_without_parent_as_invalid_path() {
        assert!(matches!(
            atomic_write(Path::new(""), b"data"),
            Err(FileError::InvalidPath(_))
        ));
    }

    #[test]
    fn atomic_write_reports_io_error_when_parent_is_a_file() {
        let dir = tempfile::tempdir().unwrap();
        let parent = dir.path().join("blocked");
        fs::write(&parent, b"existing data").unwrap();

        assert!(matches!(
            atomic_write(&parent.join("child.json"), b"new data"),
            Err(FileError::Io(_))
        ));
        assert_eq!(fs::read(parent).unwrap(), b"existing data");
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn failed_atomic_replacement_preserves_destination_and_removes_temporary_file() {
        let dir = tempfile::tempdir().unwrap();
        let destination = dir.path().join("destination");
        fs::create_dir(&destination).unwrap();
        fs::write(destination.join("keep.txt"), b"keep").unwrap();

        assert!(matches!(
            atomic_write(&destination, b"replacement"),
            Err(FileError::Io(_))
        ));
        assert_eq!(fs::read(destination.join("keep.txt")).unwrap(), b"keep");
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn child_rejects_existing_file_and_directory_symlinks_outside_root() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("root");
        // A shared string prefix must not count as containment.
        let outside = dir.path().join("root-other");
        fs::create_dir(&root).unwrap();
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("client.jar"), b"outside").unwrap();
        symlink(outside.join("client.jar"), root.join("file-link")).unwrap();
        symlink(&outside, root.join("directory-link")).unwrap();

        for path in ["file-link", "directory-link/client.jar"] {
            assert!(
                matches!(
                    child(&root, Path::new(path)),
                    Err(FileError::InvalidPath(_))
                ),
                "accepted escaping symlink: {path}"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn child_accepts_symlink_to_existing_file_inside_root() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("client.jar"), b"runtime").unwrap();
        symlink("client.jar", dir.path().join("link.jar")).unwrap();

        let path = child(dir.path(), Path::new("link.jar")).unwrap();
        assert_eq!(path, dir.path().join("link.jar"));
        assert_eq!(fs::read(path).unwrap(), b"runtime");
    }
}
