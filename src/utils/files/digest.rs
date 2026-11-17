use std::{
    fs::{self, File, Metadata},
    io::{self, Read, Write},
    path::Path,
    time::SystemTime,
};

use sha1::Sha1;
use sha2::{Digest, Sha256, Sha512};

use crate::utils::cancellation::Cancellation;

#[derive(Debug)]
pub(crate) struct Sha256Digest(String);

#[derive(Debug, thiserror::Error)]
#[error("expected a 64-digit hexadecimal SHA-256 digest")]
pub(crate) struct InvalidSha256;

impl TryFrom<&str> for Sha256Digest {
    type Error = InvalidSha256;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        if !super::hash_valid(value, 64) {
            return Err(InvalidSha256);
        }
        Ok(Self(value.to_ascii_lowercase()))
    }
}

impl From<Sha256Digest> for String {
    fn from(digest: Sha256Digest) -> Self {
        digest.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileDigests {
    pub sha256: String,
    pub sha1: String,
    pub sha512: String,
    pub size_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileStamp {
    pub size_bytes: u64,
    pub modified: Option<SystemTime>,
    pub created: Option<SystemTime>,
    #[cfg(unix)]
    pub device: u64,
    #[cfg(unix)]
    pub inode: u64,
    #[cfg(unix)]
    pub changed_seconds: i64,
    #[cfg(unix)]
    pub changed_nanoseconds: i64,
}

impl FileStamp {
    pub fn from_metadata(metadata: &Metadata) -> Self {
        #[cfg(unix)]
        use std::os::unix::fs::MetadataExt;

        Self {
            size_bytes: metadata.len(),
            modified: metadata.modified().ok(),
            created: metadata.created().ok(),
            #[cfg(unix)]
            device: metadata.dev(),
            #[cfg(unix)]
            inode: metadata.ino(),
            #[cfg(unix)]
            changed_seconds: metadata.ctime(),
            #[cfg(unix)]
            changed_nanoseconds: metadata.ctime_nsec(),
        }
    }
}

pub fn digest_file(
    path: &Path,
    cancellation: &Cancellation,
    mut on_progress: impl FnMut(u64),
) -> io::Result<(FileDigests, FileStamp)> {
    digest_regular_file(path, cancellation, |_, total| {
        on_progress(total);
        Ok(())
    })
}

pub(crate) fn copy_and_digest(
    path: &Path,
    output: &mut impl Write,
    cancellation: &Cancellation,
) -> io::Result<FileDigests> {
    let (digests, _) = digest_regular_file(path, cancellation, |bytes, _| output.write_all(bytes))?;
    Ok(digests)
}

pub(super) fn read_chunks(
    input: &mut impl Read,
    cancellation: Option<&Cancellation>,
    mut visit: impl FnMut(&[u8], u64) -> io::Result<()>,
) -> io::Result<u64> {
    let mut buffer = [0_u8; 64 * 1024];
    let mut total = 0u64;
    loop {
        if let Some(cancel) = cancellation {
            cancel.check()?;
        }
        let count = input.read(&mut buffer)?;
        if count == 0 {
            return Ok(total);
        }
        total += count as u64;
        visit(&buffer[..count], total)?;
    }
}

fn digest_regular_file(
    path: &Path,
    cancellation: &Cancellation,
    mut visit: impl FnMut(&[u8], u64) -> io::Result<()>,
) -> io::Result<(FileDigests, FileStamp)> {
    cancellation.check()?;

    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "not a regular file",
        ));
    }
    let before = FileStamp::from_metadata(&metadata);
    let mut input = File::open(path)?;
    if before != FileStamp::from_metadata(&input.metadata()?) {
        return Err(io::Error::other("file changed before hashing"));
    }

    let mut sha256 = Sha256::new();
    let mut sha1 = Sha1::new();
    let mut sha512 = Sha512::new();
    let size_bytes = read_chunks(&mut input, Some(cancellation), |bytes, total| {
        visit(bytes, total)?;
        sha256.update(bytes);
        sha1.update(bytes);
        sha512.update(bytes);
        Ok(())
    })?;

    let after = FileStamp::from_metadata(&input.metadata()?);
    let path_after = fs::symlink_metadata(path)?;
    if before != after
        || !path_after.is_file()
        || after != FileStamp::from_metadata(&path_after)
        || size_bytes != after.size_bytes
    {
        return Err(io::Error::other("file changed during hashing"));
    }

    Ok((
        FileDigests {
            sha256: format!("{:x}", sha256.finalize()),
            sha1: format!("{:x}", sha1.finalize()),
            sha512: format!("{:x}", sha512.finalize()),
            size_bytes,
        },
        after,
    ))
}
