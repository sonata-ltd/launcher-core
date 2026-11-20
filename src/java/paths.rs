use crate::utils::files;
use std::{io, path::Path};

/// Encode a managed relative placement as slash-delimited Unicode. Previously
/// persisted native separators are readable on Windows, where they are path
/// separators. On Unix a backslash could refer to a different on-disk name.
pub(super) fn portable(path: &Path) -> io::Result<String> {
    files::relative(path).map_err(io::Error::other)?;
    let spelling = path
        .to_str()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "non-Unicode Java path"))?;
    let mut encoded = String::with_capacity(spelling.len());
    for part in spelling.split(|ch| ch == '/' || cfg!(windows) && ch == '\\') {
        files::portable_name::validate_file_name(part)?;
        if !encoded.is_empty() {
            encoded.push('/');
        }
        encoded.push_str(part);
    }
    Ok(encoded)
}
