use super::{
    error::{ModloaderError, Result},
    files,
};
use std::{
    collections::HashSet,
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
};

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

    let mut zip = zip::ZipArchive::new(fs::File::open(archive)?).map_err(zip_error)?;
    let mut file = zip.by_name(entry).map_err(zip_error)?;
    let mut bytes = Vec::new();

    file.by_ref().take(limit + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(zip_error("ZIP entry too large"));
    }

    Ok(bytes)
}

pub fn extract(archive: &Path, destination: &Path, excludes: &[String]) -> Result<()> {
    fs::create_dir_all(destination)?;

    let mut zip = zip::ZipArchive::new(fs::File::open(archive)?).map_err(zip_error)?;

    if zip.len() > 20_000 {
        return Err(zip_error("too many ZIP entries"));
    }

    let mut seen = HashSet::new();
    let mut total = 0u64;

    for index in 0..zip.len() {
        let mut entry = zip.by_index(index).map_err(zip_error)?;
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

        if total > 4 * 1024 * 1024 * 1024 {
            return Err(zip_error("ZIP expands beyond limit"));
        }

        if excludes
            .iter()
            .any(|prefix| entry.name().starts_with(prefix))
        {
            continue;
        }

        let path = files::child(destination, &relative)?;
        if entry.is_dir() {
            fs::create_dir_all(&path)?;
            continue;
        }

        fs::create_dir_all(
            path.parent()
                .ok_or_else(|| zip_error("entry has no parent"))?,
        )?;

        let mut output = fs::File::create(&path)?;
        let copied = std::io::copy(&mut entry.by_ref().take(512 * 1024 * 1024 + 1), &mut output)?;

        if copied > 512 * 1024 * 1024 {
            return Err(zip_error("ZIP file too large"));
        }

        output.flush()?;
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
