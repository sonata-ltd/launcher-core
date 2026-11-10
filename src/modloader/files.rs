use super::error::{ModloaderError, Result};
use sha1::{Digest, Sha1};
use sha2::Sha256;
use std::{
    fs,
    io::{Read, Write},
    path::{Component, Path, PathBuf},
};

pub fn relative(path: &Path) -> Result<()> {
    if path.as_os_str().is_empty()
        || path
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
        || path.to_string_lossy().contains(':')
    {
        return Err(ModloaderError::Invalid(format!(
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
        return Err(ModloaderError::Invalid(
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
        .ok_or_else(|| ModloaderError::Invalid("path has no parent".into()))?;

    fs::create_dir_all(parent)?;

    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    file.write_all(bytes)?;
    file.as_file().sync_all()?;
    file.persist(path)
        .map_err(|e| ModloaderError::Io(e.error))?;

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
        assert!(relative(Path::new("../x")).is_err());
        assert!(relative(Path::new("/x")).is_err());
    }
}
