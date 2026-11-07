use super::{model::*, transfer::Cancellation};
use std::{
    fs,
    io::{self, Read, Write},
    path::{Component, Path, PathBuf},
};

const MAX_UNPACKED_SIZE: u64 = 4 * 1024 * 1024 * 1024;

struct CheckedReader<R> {
    inner: R,
    cancel: Cancellation,
}

impl<R: Read> Read for CheckedReader<R> {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        self.cancel.check().map_err(io::Error::other)?;
        self.inner.read(bytes)
    }
}

pub(super) fn extract(
    archive: &Path,
    destination: &Path,
    format: ArchiveFormat,
    cancel: &Cancellation,
) -> Result<PathBuf> {
    let run = || -> io::Result<()> {
        fs::create_dir_all(destination)?;
        let file = fs::File::open(archive)?;
        let mut total = 0u64;
        match format {
            ArchiveFormat::TarGz => {
                let reader = CheckedReader {
                    inner: flate2::read::GzDecoder::new(file),
                    cancel: cancel.clone(),
                };
                let mut archive = tar::Archive::new(reader);
                for entry in archive.entries()? {
                    cancel.check().map_err(io::Error::other)?;
                    let mut entry = entry?;
                    let path = entry.path()?.into_owned();
                    validate_path(&path)?;
                    let kind = entry.header().entry_type();
                    if !(kind.is_file()
                        || kind.is_dir()
                        || kind.is_symlink()
                        || kind.is_hard_link())
                    {
                        return Err(io::Error::other(format!(
                            "Unsupported entry: {}",
                            path.display()
                        )));
                    }
                    total = total
                        .checked_add(entry.size())
                        .ok_or_else(|| io::Error::other("Archive size overflow"))?;
                    if total > MAX_UNPACKED_SIZE {
                        return Err(io::Error::other("Archive is too large"));
                    }
                    if let Some(link) = entry.link_name()? {
                        let base = if kind.is_symlink() {
                            path.parent().unwrap_or(Path::new(""))
                        } else {
                            Path::new("")
                        };
                        validate_link(&base.join(link))?;
                    }
                    entry.set_mask(0o6000);
                    if !entry.unpack_in(destination)? {
                        return Err(io::Error::other(format!(
                            "Entry escaped destination: {}",
                            path.display()
                        )));
                    }
                }
            }
            ArchiveFormat::Zip => {
                let mut zip = zip::ZipArchive::new(file)?;
                for i in 0..zip.len() {
                    cancel.check().map_err(io::Error::other)?;
                    let mut entry = zip.by_index(i)?;
                    let relative = entry
                        .enclosed_name()
                        .ok_or_else(|| io::Error::other("Invalid ZIP path"))?;
                    validate_path(&relative)?;
                    if entry.is_symlink() {
                        return Err(io::Error::other("ZIP symlinks are not supported"));
                    }
                    total = total
                        .checked_add(entry.size())
                        .ok_or_else(|| io::Error::other("Archive size overflow"))?;
                    if total > MAX_UNPACKED_SIZE {
                        return Err(io::Error::other("Archive is too large"));
                    }
                    let path = destination.join(relative);
                    if entry.is_dir() {
                        fs::create_dir_all(path)?;
                        continue;
                    }
                    fs::create_dir_all(
                        path.parent()
                            .ok_or_else(|| io::Error::other("Missing entry parent"))?,
                    )?;
                    let mut output = fs::OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(&path)?;
                    let mut buffer = [0; 64 * 1024];
                    loop {
                        cancel.check().map_err(io::Error::other)?;
                        let n = entry.read(&mut buffer)?;
                        if n == 0 {
                            break;
                        }
                        output.write_all(&buffer[..n])?;
                    }
                    #[cfg(unix)]
                    if let Some(mode) = entry.unix_mode() {
                        use std::os::unix::fs::PermissionsExt;
                        fs::set_permissions(path, fs::Permissions::from_mode(mode & 0o777))?;
                    }
                }
            }
        }
        Ok(())
    };
    if let Err(error) = run() {
        cancel.check()?;
        return Err(ManagedJavaError::new(
            "extract",
            format!(
                "Archive: {}\nDestination: {}\n{error}",
                archive.display(),
                destination.display()
            ),
        ));
    }
    cancel.check()?;
    find_home(destination).map_err(|e| ManagedJavaError::new("extract", e))
}

fn validate_path(path: &Path) -> io::Result<()> {
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
    Ok(())
}

fn validate_link(path: &Path) -> io::Result<()> {
    let mut depth = 0usize;
    for component in path.components() {
        match component {
            Component::Normal(_) => depth += 1,
            Component::CurDir => (),
            Component::ParentDir if depth > 0 => depth -= 1,
            _ => return Err(io::Error::other(format!("Unsafe link: {}", path.display()))),
        }
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
