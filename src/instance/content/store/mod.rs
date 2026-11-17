use std::{
    fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
};

use crate::utils::{
    cancellation::Cancellation,
    files::digest::{digest_file, FileDigests, FileStamp},
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
        let staging = self.root.join("staging");
        fs::create_dir_all(&staging)?;

        if !fs::symlink_metadata(source)?.file_type().is_file() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "source is not regular",
            ));
        }

        let mut input = fs::File::open(source)?;
        let before = FileStamp::from_metadata(&input.metadata()?);

        let mut temporary = tempfile::NamedTempFile::new_in(&staging)?;
        let mut buffer = [0_u8; 64 * 1024];

        loop {
            cancellation.check()?;

            let count = input.read(&mut buffer)?;

            if count == 0 {
                break;
            }

            temporary.write_all(&buffer[..count])?;
        }

        if before != FileStamp::from_metadata(&input.metadata()?)
            || before != FileStamp::from_metadata(&fs::metadata(source)?)
        {
            return Err(io::Error::other("source changed during import"));
        }
        temporary.as_file().sync_all()?;

        let (digests, _) = digest_file(temporary.path(), cancellation, |_| {})?;
        if expected_sha256.is_some_and(|expected| !expected.eq_ignore_ascii_case(&digests.sha256)) {
            return Err(io::Error::other("source differs from preview"));
        }

        let destination = self.path_for(&digests.sha256)?;
        let parent = destination
            .parent()
            .ok_or_else(|| io::Error::other("missing parent"))?;

        fs::create_dir_all(parent)?;
        cancellation.check()?;

        match temporary.persist_noclobber(&destination) {
            Ok(_) => {}
            Err(e) if e.error.kind() == io::ErrorKind::AlreadyExists => {
                let (existing_digests, _) = digest_file(&destination, cancellation, |_| {})?;

                if existing_digests != digests {
                    return Err(io::Error::other("existing CAS object is corrupt"));
                }
            }
            Err(e) => return Err(e.error),
        }

        Ok(StoredObject {
            digests,
            path: destination,
        })
    }
}

pub mod storage;

pub(in crate::instance::content) mod legacy;
