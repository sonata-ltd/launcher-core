use std::{
    ffi::OsStr,
    io::ErrorKind,
    path::{Path, PathBuf},
    time::Duration,
};

use async_std::{fs, stream::StreamExt};

pub async fn sweep_partials(root: &Path, min_age: Duration) -> usize {
    let mut removed = 0usize;
    let mut queue: Vec<PathBuf> = vec![root.to_path_buf()];

    while let Some(dir) = queue.pop() {
        let mut entries = match fs::read_dir(&dir).await {
            Ok(entries) => entries,
            Err(e) if e.kind() == ErrorKind::NotFound => continue,
            Err(e) => {
                tracing::warn!(path = %dir.display(), error = %e, "sweep: cannot read directory");
                continue;
            }
        };

        while let Some(entry) = entries.next().await {
            let entry = match entry {
                Ok(entry) => entry,
                Err(e) => {
                    tracing::warn!(path = %dir.display(), error = %e, "sweep: unreadable entry");
                    continue;
                }
            };

            let path: PathBuf = entry.path().into();

            let file_type = match entry.file_type().await {
                Ok(ft) => ft,
                Err(e) => {
                    tracing::warn!(path = %path.display(), error = %e, "sweep: cannot stat");
                    continue;
                }
            };

            if file_type.is_dir() {
                queue.push(path);
                continue;
            }

            if !file_type.is_file() || !is_partial(&path) {
                continue;
            }

            let modified = match entry.metadata().await.and_then(|m| m.modified()) {
                Ok(modified) => modified,
                Err(e) => {
                    tracing::warn!(path = %path.display(), error = %e, "sweep: no mtime");
                    continue;
                }
            };

            match modified.elapsed() {
                Ok(age) if age >= min_age => {}
                _ => continue,
            }

            match fs::remove_file(&path).await {
                Ok(()) => {
                    tracing::debug!(path = %path.display(), "sweep: removed partial download");
                    removed += 1;
                }
                Err(e) if e.kind() == ErrorKind::NotFound => {}
                Err(e) => {
                    tracing::warn!(path = %path.display(), error = %e, "sweep: cannot remove")
                }
            }
        }
    }

    removed
}

pub fn is_partial(path: &Path) -> bool {
    if path.extension() != Some(OsStr::new("part")) {
        return false;
    }

    let Some(name) = path.file_name() else {
        return false;
    };

    if !name.as_encoded_bytes().starts_with(b".") {
        return false;
    }

    let Some(stem) = path.file_stem() else {
        return false;
    };

    Path::new(stem)
        .extension()
        .and_then(OsStr::to_str)
        .is_some_and(|id| id.len() == 32 && id.bytes().all(|b| b.is_ascii_hexdigit()))
}
