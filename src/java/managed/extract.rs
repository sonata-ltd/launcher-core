use super::model::*;
use crate::utils::{cancellation::Cancellation, files::directory::Directory};
use std::{
    collections::HashSet,
    fs,
    io::{self, Read, Write},
    path::{Component, Path, PathBuf},
    sync::Arc,
};

struct ArchiveLimits {
    entries: usize,
    bytes: u64,
}

const LIMITS: ArchiveLimits = ArchiveLimits {
    entries: 32_768,
    bytes: 4 * 1024 * 1024 * 1024,
};

struct CheckedReader<R> {
    inner: R,
    cancel: Cancellation,
}

impl<R: Read> Read for CheckedReader<R> {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        self.cancel.check()?;
        self.inner.read(bytes)
    }
}

#[cfg(test)]
pub(super) fn extract(
    archive: &Path,
    destination: &Path,
    format: ArchiveFormat,
    cancel: &Cancellation,
) -> Result<PathBuf> {
    extract_bounded(archive, destination, format, cancel, LIMITS)
}

#[cfg(test)]
fn extract_bounded(
    archive: &Path,
    destination: &Path,
    format: ArchiveFormat,
    cancel: &Cancellation,
    limits: ArchiveLimits,
) -> Result<PathBuf> {
    cancel.check()?;
    let directory = Directory::ensure_path(destination)
        .map_err(|error| archive_error(archive, destination, error))?;
    extract_into(archive, &directory, format, cancel, limits)
}

pub(super) fn extract_in(
    archive: &Path,
    destination: &Arc<Directory>,
    format: ArchiveFormat,
    cancel: &Cancellation,
) -> Result<PathBuf> {
    extract_into(archive, destination, format, cancel, LIMITS)
}

fn extract_into(
    archive: &Path,
    destination: &Arc<Directory>,
    format: ArchiveFormat,
    cancel: &Cancellation,
    limits: ArchiveLimits,
) -> Result<PathBuf> {
    let run = || -> io::Result<()> {
        cancel.check()?;
        destination.verify_location()?;
        let file = fs::File::open(archive)?;
        let mut total = 0u64;
        match format {
            ArchiveFormat::TarGz => {
                let reader = CheckedReader {
                    inner: flate2::read::GzDecoder::new(file),
                    cancel: cancel.clone(),
                };
                extract_tar(reader, destination, cancel, &limits)?;
            }
            ArchiveFormat::Zip => {
                let mut zip = zip::ZipArchive::new(file)?;
                if zip.len() > limits.entries {
                    return Err(io::Error::other("too many Java archive entries"));
                }
                let mut written = 0u64;
                for i in 0..zip.len() {
                    cancel.check()?;
                    let mut entry = zip.by_index(i)?;
                    let relative = entry
                        .enclosed_name()
                        .ok_or_else(|| io::Error::other("Invalid ZIP path"))?;
                    let relative = archive_path(&relative)?;
                    if entry.is_symlink() {
                        return Err(io::Error::other("ZIP symlinks are not supported"));
                    }
                    total = total
                        .checked_add(entry.size())
                        .ok_or_else(|| io::Error::other("Archive size overflow"))?;
                    if total > limits.bytes {
                        return Err(io::Error::other("Archive is too large"));
                    }
                    if entry.is_dir() {
                        if !relative.as_os_str().is_empty() {
                            destination.create_relative(&relative)?;
                        }
                        continue;
                    }
                    let parent = destination.create_entry_parent(&relative)?;
                    let mut output = parent.create_file(file_name(&relative)?)?;
                    write_entry(&mut entry, &mut output, cancel, &mut written, limits.bytes)?;
                    #[cfg(unix)]
                    if let Some(mode) = entry.unix_mode() {
                        use std::os::unix::fs::PermissionsExt;
                        output.set_permissions(fs::Permissions::from_mode(mode & 0o777))?;
                    }
                }
            }
        }
        destination.verify_location()
    };
    if let Err(error) = run() {
        cancel.check()?;
        return Err(archive_error(archive, destination.path(), error));
    }
    cancel.check()?;
    find_home(destination.path())
        .map_err(|e| ManagedJavaError::caused_by(super::ManagedJavaErrorKind::Extract, e))
}

fn archive_error(archive: &Path, destination: &Path, error: io::Error) -> ManagedJavaError {
    let mut error = ManagedJavaError::caused_by(super::ManagedJavaErrorKind::Extract, error);
    error.details = format!(
        "Archive: {}\nDestination: {}\n{}",
        archive.display(),
        destination.display(),
        error.details,
    );
    error
}

fn extract_tar(
    reader: impl Read,
    root: &Arc<Directory>,
    cancel: &Cancellation,
    limits: &ArchiveLimits,
) -> io::Result<()> {
    let mut archive = tar::Archive::new(reader);
    let mut size = 0u64;
    let mut written = 0u64;
    let mut files = HashSet::new();
    let mut symlinks = Vec::new();
    #[cfg(unix)]
    let mut directories = Vec::new();

    for (index, entry) in archive.entries()?.enumerate() {
        if index >= limits.entries {
            return Err(io::Error::other("too many Java archive entries"));
        }
        cancel.check()?;
        let mut entry = entry?;
        let path = archive_path(&entry.path()?)?;
        size = size
            .checked_add(entry.size())
            .filter(|size| *size <= limits.bytes)
            .ok_or_else(|| io::Error::other("Archive is too large"))?;
        let kind = entry.header().entry_type();
        if kind.is_dir() {
            if !path.as_os_str().is_empty() {
                root.create_relative(&path)?;
                #[cfg(unix)]
                directories.push((path, entry.header().mode()?));
            }
            continue;
        }
        let name = file_name(&path)?;
        let parent = root.create_entry_parent(&path)?;
        if kind.is_file() {
            let mut output = parent.create_file(name)?;
            write_entry(&mut entry, &mut output, cancel, &mut written, limits.bytes)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                output
                    .set_permissions(fs::Permissions::from_mode(entry.header().mode()? & 0o1777))?;
            }
            if let Some(modified) = entry.header().mtime().ok().and_then(|seconds| {
                std::time::UNIX_EPOCH.checked_add(std::time::Duration::from_secs(seconds))
            }) {
                output.set_modified(modified)?;
            }
            files.insert(path);
        } else if kind.is_symlink() || kind.is_hard_link() {
            let link = entry
                .link_name()?
                .ok_or_else(|| io::Error::other("Missing archive link target"))?;
            if kind.is_symlink() {
                let base = path.parent().unwrap_or(Path::new(""));
                let target = link_target(base, &link)?;
                symlinks.push((path, target));
            } else {
                let source = link_target(Path::new(""), &link)?;
                if !files.contains(&source) {
                    return Err(io::Error::other(
                        "Archive hard link target is not an extracted file",
                    ));
                }
                let source_parent = root.create_entry_parent(&source)?;
                source_parent.link_file(file_name(&source)?, &parent, name)?;
                files.insert(path);
            }
        } else {
            return Err(io::Error::other(format!(
                "Unsupported entry: {}",
                path.display()
            )));
        }
    }

    write_symlinks(root, &symlinks, cancel)?;
    #[cfg(unix)]
    for (path, mode) in directories.into_iter().rev() {
        use std::os::unix::fs::PermissionsExt;
        cancel.check()?;
        root.open_relative(&path)?
            .set_permissions(fs::Permissions::from_mode(mode & 0o1777))?;
    }
    Ok(())
}

fn file_name(path: &Path) -> io::Result<&std::ffi::OsStr> {
    path.file_name()
        .ok_or_else(|| io::Error::other("Missing archive filename"))
}

fn write_entry(
    input: &mut impl Read,
    output: &mut fs::File,
    cancel: &Cancellation,
    written: &mut u64,
    limit: u64,
) -> io::Result<()> {
    let mut buffer = [0; 64 * 1024];
    loop {
        cancel.check()?;
        let bytes = input.read(&mut buffer)?;
        if bytes == 0 {
            return Ok(());
        }
        *written = written
            .checked_add(bytes as u64)
            .filter(|written| *written <= limit)
            .ok_or_else(|| io::Error::other("Java archive output exceeds byte limit"))?;
        output.write_all(&buffer[..bytes])?;
    }
}

fn archive_path(path: &Path) -> io::Result<PathBuf> {
    if path
        .components()
        .any(|c| !matches!(c, Component::Normal(_) | Component::CurDir))
        || path.as_os_str().to_string_lossy().contains('\\')
    {
        return Err(io::Error::other(format!(
            "Unsafe archive path: {}",
            path.display()
        )));
    }
    Ok(path
        .components()
        .filter_map(|component| match component {
            Component::Normal(name) => Some(name),
            _ => None,
        })
        .collect())
}

fn link_target(base: &Path, link: &Path) -> io::Result<PathBuf> {
    if link.as_os_str().is_empty() || link.to_string_lossy().contains(['\\', ':']) {
        return Err(io::Error::other("Invalid archive link target"));
    }
    let mut target = base.to_owned();
    for component in link.components() {
        match component {
            Component::Normal(name) => target.push(name),
            Component::CurDir => (),
            Component::ParentDir if target.pop() => (),
            _ => return Err(io::Error::other(format!("Unsafe link: {}", link.display()))),
        }
    }
    Ok(target)
}

fn write_symlinks(
    root: &Arc<Directory>,
    links: &[(PathBuf, PathBuf)],
    cancel: &Cancellation,
) -> io::Result<()> {
    let names: HashSet<&Path> = links.iter().map(|(name, _)| name.as_path()).collect();
    for (path, target) in links {
        cancel.check()?;
        if let Some(parent) = target
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            if parent
                .ancestors()
                .any(|component| names.contains(component))
            {
                return Err(io::Error::other("Archive link target has a linked parent"));
            }
            match root.open_relative(parent) {
                Ok(_) => (),
                Err(error) if error.kind() == io::ErrorKind::NotFound => (),
                Err(error) => return Err(error),
            }
        }
        if !target.as_os_str().is_empty() && !names.contains(target.as_path()) {
            match fs::symlink_metadata(root.path().join(target)) {
                Ok(metadata) if metadata.is_file() || metadata.is_dir() => (),
                Ok(_) => return Err(io::Error::other("Archive link target is redirected")),
                Err(error) if error.kind() == io::ErrorKind::NotFound => (),
                Err(error) => return Err(error),
            }
        }
        let parent = root.create_entry_parent(path)?;
        let base = path.parent().unwrap_or(Path::new(""));
        let common = base
            .components()
            .zip(target.components())
            .take_while(|(left, right)| left == right)
            .count();
        let mut relative: PathBuf = base.components().skip(common).map(|_| "..").collect();
        relative.extend(target.components().skip(common));
        if relative.as_os_str().is_empty() {
            relative.push(".");
        }
        parent.create_symlink(&relative, file_name(path)?)?;
    }
    Ok(())
}

pub(super) fn executable(home: &Path) -> PathBuf {
    home.join("bin")
        .join(if cfg!(windows) { "java.exe" } else { "java" })
}

fn find_home(root: &Path) -> io::Result<PathBuf> {
    let mut candidates = Vec::new();
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let path = entry.path();
        for home in [path.clone(), path.join("Contents/Home")] {
            let exec = executable(&home);
            if exec.is_file() && fs::canonicalize(&exec)?.starts_with(fs::canonicalize(root)?) {
                candidates.push(home);
            }
        }
    }
    if candidates.len() != 1 {
        return Err(io::Error::other(format!(
            "Expected one Java home, found {}",
            candidates.len()
        )));
    }
    Ok(candidates.remove(0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zip_entry_budget_is_enforced_before_creating_outputs() {
        let root = tempfile::tempdir().unwrap();
        let archive = root.path().join("runtime.zip");
        let mut writer = zip::ZipWriter::new(fs::File::create(&archive).unwrap());
        for name in ["jre/bin/java", "jre/info"] {
            writer
                .start_file(name, zip::write::SimpleFileOptions::default())
                .unwrap();
            writer.write_all(b"fixture").unwrap();
        }
        writer.finish().unwrap();
        let destination = root.path().join("unpacked");
        let error = extract_bounded(
            &archive,
            &destination,
            ArchiveFormat::Zip,
            &Cancellation::new(),
            ArchiveLimits {
                entries: 1,
                bytes: 1024,
            },
        )
        .unwrap_err();
        assert!(error.diagnostic().contains("too many Java archive entries"));
        assert!(fs::read_dir(destination).unwrap().next().is_none());
    }
}
