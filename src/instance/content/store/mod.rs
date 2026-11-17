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

#[derive(Debug)]
pub struct StoredObject {
    pub digests: FileDigests,
    pub path: PathBuf,
}

impl ObjectStore {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
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

    pub(in crate::instance::content) fn import_checked<E: From<io::Error>>(
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

        Ok(StoredObject {
            digests,
            path: destination,
        })
    }
}

pub mod storage;

pub(in crate::instance::content) mod legacy;
