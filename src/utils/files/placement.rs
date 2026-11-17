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

/// Copy a regular directory tree, replacing regular destination files atomically.
/// Existing destination directories are merged. Links and special files are rejected.
pub(crate) fn copy_tree_replace(
    source: &Path,
    destination: &Path,
    cancel: &Cancellation,
) -> io::Result<()> {
    let mut pending = vec![(source.to_owned(), destination.to_owned())];
    while let Some((source, destination)) = pending.pop() {
        cancel.check()?;
        require_kind(&source, true, false)?;
        require_kind(&destination, true, true)?;
        fs::create_dir_all(&destination)?;

        for entry in fs::read_dir(&source)? {
            cancel.check()?;
            let entry = entry?;
            let kind = entry.file_type()?;
            let target = destination.join(entry.file_name());
            if kind.is_dir() {
                pending.push((entry.path(), target));
            } else if kind.is_file() {
                require_kind(&target, false, true)?;
                copy_replace(&entry.path(), &target, cancel)?;
            } else {
                return Err(invalid_tree_entry(&entry.path()));
            }
        }
    }
    Ok(())
}

fn require_kind(path: &Path, directory: bool, allow_missing: bool) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if (directory && metadata.is_dir()) || (!directory && metadata.is_file()) => {
            Ok(())
        }
        Ok(_) => Err(invalid_tree_entry(path)),
        Err(error) if allow_missing && error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn invalid_tree_entry(path: &Path) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        format!("unexpected file type in directory tree: {}", path.display()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tree_copy_replaces_files_and_respects_cancellation() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("source");
        let destination = root.path().join("destination");
        for dir in [&source, &destination] {
            fs::create_dir_all(dir.join("nested")).unwrap();
        }
        fs::write(source.join("nested/a.jar"), b"new").unwrap();
        fs::write(destination.join("nested/a.jar"), b"old").unwrap();

        let cancel = Cancellation::new();
        copy_tree_replace(&source, &destination, &cancel).unwrap();
        assert_eq!(fs::read(destination.join("nested/a.jar")).unwrap(), b"new");

        cancel.cancel();
        fs::write(source.join("nested/a.jar"), b"newer").unwrap();
        assert_eq!(
            copy_tree_replace(&source, &destination, &cancel)
                .unwrap_err()
                .kind(),
            io::ErrorKind::Interrupted
        );
        assert_eq!(fs::read(destination.join("nested/a.jar")).unwrap(), b"new");
    }

    #[cfg(unix)]
    #[test]
    fn tree_copy_rejects_source_and_destination_symlinks() {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("source");
        let target = root.path().join("target");
        let outside = root.path().join("outside");
        for dir in [&source, &target, &outside] {
            fs::create_dir(dir).unwrap();
        }
        fs::write(outside.join("a.jar"), b"original").unwrap();
        symlink(outside.join("a.jar"), source.join("a.jar")).unwrap();
        let cancel = Cancellation::new();
        assert_eq!(
            copy_tree_replace(&source, &target, &cancel)
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidInput
        );

        fs::remove_file(source.join("a.jar")).unwrap();
        fs::write(source.join("a.jar"), b"replacement").unwrap();
        symlink(outside.join("a.jar"), target.join("a.jar")).unwrap();
        assert_eq!(
            copy_tree_replace(&source, &target, &cancel)
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidInput
        );
        assert_eq!(fs::read(outside.join("a.jar")).unwrap(), b"original");
    }
}
