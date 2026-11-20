use crate::utils::{
    cancellation::Cancellation,
    files::{self, directory::Directory},
};

use super::error::{ModloaderError, Result};
use std::{
    collections::HashSet,
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
};

pub(super) async fn stage_embedded_libraries(
    source: PathBuf,
    destination: PathBuf,
    cancel: &Cancellation,
) -> Result<()> {
    let token = cancel.clone();
    let result = async_std::task::spawn_blocking(move || match fs::symlink_metadata(&source) {
        Ok(_) => files::placement::copy_tree_replace(&source, &destination, &token),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    })
    .await;
    cancel.check()?;
    Ok(result?)
}

fn zip_error(e: impl std::fmt::Display) -> ModloaderError {
    ModloaderError::Invalid(e.to_string())
}

fn name(raw: &str) -> Result<PathBuf> {
    if raw.contains('\\') {
        return Err(zip_error("backslash in ZIP entry"));
    }

    let path = PathBuf::from(raw.trim_end_matches('/'));
    files::relative(&path)?;

    Ok(path)
}

pub fn read(archive: &Path, resource: &str, limit: u64) -> Result<Vec<u8>> {
    let entry = resource.strip_prefix('/').unwrap_or(resource);
    name(entry)?;

    let mut zip = zip::ZipArchive::new(fs::File::open(archive)?)?;
    let mut file = zip.by_name(entry)?;
    let mut bytes = Vec::new();

    file.by_ref()
        .take(limit.saturating_add(1))
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(zip_error("ZIP entry too large"));
    }

    Ok(bytes)
}

pub fn extract(archive: &Path, destination: &Path, excludes: &[String]) -> Result<()> {
    const MAX_BYTES: u64 = 4 * 1024 * 1024 * 1024;
    const MAX_ENTRY_BYTES: u64 = 512 * 1024 * 1024;
    fs::create_dir_all(destination)?;
    let destination = Directory::open(destination)?;

    let mut zip = zip::ZipArchive::new(fs::File::open(archive)?)?;

    if zip.len() > 20_000 {
        return Err(zip_error("too many ZIP entries"));
    }

    let mut seen = HashSet::new();
    let mut total = 0u64;
    let mut remaining = MAX_BYTES;

    for index in 0..zip.len() {
        let mut entry = zip.by_index(index)?;
        let relative = name(entry.name())?;

        if !seen.insert(relative.clone()) {
            return Err(zip_error("duplicate ZIP entry"));
        }

        if entry.unix_mode().is_some_and(|m| m & 0o170000 == 0o120000) {
            return Err(zip_error("ZIP symlink"));
        }

        total = total
            .checked_add(entry.size())
            .ok_or_else(|| zip_error("ZIP size overflow"))?;

        if total > MAX_BYTES {
            return Err(zip_error("ZIP expands beyond limit"));
        }

        if excludes
            .iter()
            .any(|prefix| entry.name().starts_with(prefix))
        {
            continue;
        }

        if entry.is_dir() {
            destination.create_relative(&relative)?;
            continue;
        }

        let parent = destination.create_entry_parent(&relative)?;
        let name = relative
            .file_name()
            .ok_or_else(|| zip_error("entry has no filename"))?;
        parent.with_staged_file(
            |output| -> Result<()> {
                let limit = remaining.min(MAX_ENTRY_BYTES);
                let copied = std::io::copy(&mut entry.by_ref().take(limit), output)?;
                if entry.read(&mut [0; 1])? != 0 {
                    return Err(zip_error(
                        "ZIP output exceeds the file or archive byte limit",
                    ));
                }
                remaining -= copied;
                output.flush()?;
                Ok(())
            },
            |staged, ()| {
                match fs::symlink_metadata(parent.path().join(name)) {
                    Ok(metadata) if !metadata.is_file() => {
                        return Err(zip_error("ZIP output is not a regular file"));
                    }
                    Ok(_) => (),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
                    Err(error) => return Err(error.into()),
                }
                parent.replace(staged, name)?;
                Ok(())
            },
        )?;
    }

    Ok(())
}

pub fn main_class(jar: &Path) -> Result<String> {
    let raw =
        String::from_utf8(read(jar, "META-INF/MANIFEST.MF", 1024 * 1024)?).map_err(zip_error)?;
    let mut lines: Vec<String> = Vec::new();

    for line in raw.lines() {
        if line.is_empty() {
            break;
        }

        if let Some(continuation) = line.strip_prefix(' ') {
            lines
                .last_mut()
                .ok_or_else(|| zip_error("invalid JAR manifest continuation"))?
                .push_str(continuation);
        } else {
            lines.push(line.into());
        }
    }

    lines
        .into_iter()
        .find_map(|line| {
            line.split_once(':').and_then(|(key, value)| {
                key.eq_ignore_ascii_case("Main-Class")
                    .then(|| value.trim().to_owned())
            })
        })
        .filter(|s| !s.is_empty())
        .ok_or_else(|| zip_error("processer JAR has Main-Class"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merging_archives_replaces_the_directory_entry_without_changing_linked_bytes() {
        let root = tempfile::tempdir().unwrap();
        let destination = root.path().join("natives");
        let previous = root.path().join("previous");
        fs::create_dir_all(destination.join("nested")).unwrap();
        fs::write(&previous, b"previous").unwrap();
        let output = destination.join("nested/library");
        fs::hard_link(&previous, &output).unwrap();
        let archive = root.path().join("natives.jar");
        let mut zip = zip::ZipWriter::new(fs::File::create(&archive).unwrap());
        zip.start_file("nested/library", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(b"new library").unwrap();
        zip.finish().unwrap();

        extract(&archive, &destination, &[]).unwrap();

        assert_eq!(fs::read(previous).unwrap(), b"previous");
        assert_eq!(fs::read(output).unwrap(), b"new library");
    }
}
