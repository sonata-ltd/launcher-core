use std::{
    fs::{self, File, Metadata},
    io::{self, Read},
    path::Path,
    time::SystemTime,
};

use sha1::Sha1;
use sha2::{Digest, Sha256, Sha512};

use crate::utils::cancellation::Cancellation;

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
    cancellation.check()?;

    if !fs::symlink_metadata(path)?.file_type().is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "not a regular file",
        ));
    }

    let mut input = File::open(path)?;
    let before = FileStamp::from_metadata(&input.metadata()?);

    let mut sha256 = Sha256::new();
    let mut sha1 = Sha1::new();
    let mut sha512 = Sha512::new();
    let mut size_bytes = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];

    loop {
        cancellation.check()?;

        let count = input.read(&mut buffer)?;

        if count == 0 {
            break;
        }

        let bytes = &buffer[..count];

        sha256.update(bytes);
        sha1.update(bytes);
        sha512.update(bytes);

        size_bytes += count as u64;
        on_progress(size_bytes);
    }

    let after = FileStamp::from_metadata(&input.metadata()?);
    let path_after = FileStamp::from_metadata(&fs::metadata(path)?);

    if before != after || after != path_after || size_bytes != after.size_bytes {
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
