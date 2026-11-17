use std::{
    fs::{self, File, OpenOptions},
    io::{self, Write},
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

    let result = copy_contents(&mut input, &mut output, cancellation);

    drop(output);

    if result.is_err() {
        let _ = fs::remove_file(destination);
    }

    result
}

/// Replace only after the sibling staging file has been fully written.
pub(crate) fn copy_replace(
    source: &Path,
    destination: &Path,
    cancellation: &Cancellation,
) -> io::Result<()> {
    cancellation.check()?;
    let parent = destination
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let mut input = File::open(source)?;
    let mut staging = tempfile::NamedTempFile::new_in(parent)?;
    copy_contents(&mut input, staging.as_file_mut(), cancellation)?;
    cancellation.check()?;
    staging.persist(destination).map_err(|error| error.error)?;
    Ok(())
}

fn copy_contents(
    input: &mut File,
    output: &mut File,
    cancellation: &Cancellation,
) -> io::Result<()> {
    super::digest::read_chunks(input, Some(cancellation), |bytes, _| {
        output.write_all(bytes)
    })?;
    output.sync_all()
}
