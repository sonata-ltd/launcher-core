use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::Path,
};

use crate::utils::cancellation::Cancellation;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlacementKind {
    Hardlink,
    Copy,
}

#[derive(Debug, Clone, Copy)]
pub enum CopyFallback {
    Never,
    AllowedAfterLinkFailure,
}

#[derive(Debug)]
pub struct PlacementResult {
    pub kind: PlacementKind,
    pub hardlink_error: Option<io::Error>,
}

pub fn place_new(
    source: &Path,
    destination: &Path,
    fallback: CopyFallback,
    cancellation: &Cancellation,
) -> io::Result<PlacementResult> {
    cancellation.check()?;

    if !fs::symlink_metadata(source)?.file_type().is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "source is not regular",
        ));
    }

    match fs::hard_link(source, destination) {
        Ok(()) => Ok(PlacementResult {
            kind: PlacementKind::Hardlink,
            hardlink_error: None,
        }),
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Err(e),
        Err(e) => {
            if matches!(fallback, CopyFallback::Never) {
                return Err(e);
            }

            copy_new(source, destination, cancellation)?;

            Ok(PlacementResult {
                kind: PlacementKind::Copy,
                hardlink_error: Some(e),
            })
        }
    }
}

pub fn copy_new(source: &Path, destination: &Path, cancellation: &Cancellation) -> io::Result<()> {
    let mut input = File::open(source)?;
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)?;

    let result = (|| {
        let mut buffer = [0_u8; 64 * 1024];

        loop {
            cancellation.check()?;

            let count = input.read(&mut buffer)?;

            if count == 0 {
                break;
            }

            output.write_all(&buffer[..count])?;
        }

        output.sync_all()
    })();

    drop(output);

    if result.is_err() {
        let _ = fs::remove_file(destination);
    }

    result
}
