//! Публикация нового файла без перезаписи назначения. Общая операция не знает
//! ни о модах, ни о SQLite. Источник обязан быть приватным staging-файлом.
use std::{io, path::PathBuf};

#[allow(deprecated)] // from_path поддерживается минимальной tempfile 3.21 из Cargo.lock.
pub fn publish_new(source: PathBuf, destination: PathBuf) -> io::Result<()> {
    tempfile::TempPath::from_path(source)
        .persist_noclobber(destination)
        .map_err(|error| error.error)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    #[test]
    fn concurrent_external_destination_is_never_overwritten() {
        let directory = tempfile::tempdir().unwrap();
        let staged = directory.path().join("staged");
        let destination = directory.path().join("mod.jar");
        fs::write(&staged, b"downloaded").unwrap();
        fs::write(&destination, b"user file").unwrap();
        assert!(publish_new(staged, destination.clone()).is_err());
        assert_eq!(fs::read(destination).unwrap(), b"user file");
    }
}
