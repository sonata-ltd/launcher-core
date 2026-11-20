use std::{
    ffi::OsStr,
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::Path,
};

use super::directory::Directory;
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
#[cfg(test)]
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
    let name = destination.file_name().ok_or(io::ErrorKind::InvalidInput)?;
    let directory = Directory::ensure_path(parent)?;
    copy_replace_in(source, &directory, name, cancellation)
}

pub(crate) fn copy_replace_in(
    source: &Path,
    destination: &Directory,
    name: &OsStr,
    cancel: &Cancellation,
) -> io::Result<()> {
    cancel.check()?;
    copy_into(File::open(source)?, destination, name, cancel)
}

fn copy_into(
    mut input: File,
    destination: &Directory,
    name: &OsStr,
    cancel: &Cancellation,
) -> io::Result<()> {
    destination.with_staged_file(
        |output| copy_contents(&mut input, output, cancel),
        |temporary, ()| {
            cancel.check()?;
            destination.require_regular_or_missing(name)?;
            destination.replace(temporary, name)
        },
    )
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
    cancel.check()?;
    let mut pending = vec![(
        Directory::open(source)?,
        Directory::ensure_path(destination)?,
    )];
    while let Some((source, destination)) = pending.pop() {
        cancel.check()?;
        source.verify_location()?;
        destination.verify_location()?;

        for entry in fs::read_dir(source.path())? {
            cancel.check()?;
            let entry = entry?;
            let kind = entry.file_type()?;
            let name = entry.file_name();
            if kind.is_dir() {
                pending.push((
                    source.open_relative(Path::new(&name))?,
                    destination.create_child(&name)?,
                ));
            } else if kind.is_file() {
                copy_into(source.open_file(&name)?, &destination, &name, cancel)?;
            } else {
                return Err(invalid_tree_entry(&entry.path()));
            }
        }
    }
    Ok(())
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

    #[cfg(any(unix, windows))]
    #[test]
    fn copy_replace_refuses_a_redirected_destination_directory() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("new.jar");
        let outside = root.path().join("other");
        let destination = root.path().join("libraries");
        fs::create_dir(&outside).unwrap();
        fs::write(&source, b"new bytes").unwrap();
        fs::write(outside.join("library.jar"), b"preserve these bytes").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside, &destination).unwrap();
        #[cfg(windows)]
        assert!(std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&destination)
            .arg(&outside)
            .status()
            .unwrap()
            .success());

        let result = copy_replace(
            &source,
            &destination.join("library.jar"),
            &Cancellation::new(),
        );

        assert_eq!(
            fs::read(outside.join("library.jar")).unwrap(),
            b"preserve these bytes"
        );
        assert!(result.is_err());
    }

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
