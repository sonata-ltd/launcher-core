use std::{
    ffi::OsString,
    fs, io,
    path::{Path, PathBuf},
};

use crate::utils::files::digest::FileStamp;

pub use crate::instance::content::model::ContentKind;

#[derive(Debug)]
pub enum ObservedType {
    Archive(FileStamp),
    LocalDirectory,
    SymbolicLink,
}

#[derive(Debug)]
pub struct ObservedEntry {
    pub name: OsString,
    pub relative_path: PathBuf,
    pub kind: ContentKind,
    pub mod_enabled: Option<bool>,
    pub entry_type: ObservedType,
}

#[derive(Debug)]
pub struct InventoryScope {
    pub entries: Vec<ObservedEntry>,
    pub directory_exists: bool,
}

pub fn inventory_scope(instance_root: &Path, kind: ContentKind) -> io::Result<InventoryScope> {
    let relative_directory = Path::new(match kind {
        ContentKind::Mod => "mods",
        ContentKind::ResourcePack => "resourcepacks",
        ContentKind::ShaderPack => "shaderpacks",
    });

    let directory = instance_root.join(relative_directory);
    let directory_metadata = match fs::symlink_metadata(&directory) {
        Ok(metadata) => metadata,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            fs::metadata(instance_root)?;

            return Ok(InventoryScope {
                entries: Vec::new(),
                directory_exists: false,
            });
        }
        Err(e) => return Err(e),
    };

    if !directory_metadata.file_type().is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "content directory is not regular",
        ));
    }

    let mut entries = Vec::new();
    for entry in fs::read_dir(&directory)? {
        let entry = entry?;
        let name = entry.file_name();
        let relative_path = relative_directory.join(&name);
        let metadata = fs::symlink_metadata(entry.path())?;
        let file_type = metadata.file_type();

        if file_type.is_symlink() {
            entries.push(ObservedEntry {
                name,
                relative_path,
                kind,
                mod_enabled: None,
                entry_type: ObservedType::SymbolicLink,
            });

            continue;
        }

        if file_type.is_dir() && kind != ContentKind::Mod {
            entries.push(ObservedEntry {
                name,
                relative_path,
                kind,
                mod_enabled: None,
                entry_type: ObservedType::LocalDirectory,
            });

            continue;
        }

        let suffix = entry
            .path()
            .extension()
            .and_then(|value| value.to_str())
            .map(str::to_ascii_lowercase);

        let mod_enabled = if kind == ContentKind::Mod {
            if suffix.as_deref() == Some("jar") {
                Some(true)
            } else if suffix.as_deref() == Some("disabled")
                && entry.path().file_stem().is_some_and(|stem| {
                    Path::new(stem)
                        .extension()
                        .is_some_and(|extension| extension.eq_ignore_ascii_case("jar"))
                })
            {
                Some(false)
            } else {
                continue;
            }
        } else {
            if suffix.as_deref() != Some("zip") {
                continue;
            }

            None
        };

        if file_type.is_file() {
            entries.push(ObservedEntry {
                name,
                relative_path,
                kind,
                mod_enabled,
                entry_type: ObservedType::Archive(FileStamp::from_metadata(&metadata)),
            });
        }
    }

    Ok(InventoryScope {
        entries,
        directory_exists: true,
    })
}

use sha2::Digest;

pub(crate) async fn signature(
    root: std::path::PathBuf,
) -> crate::instance::content::error::Result<String> {
    async_std::task::spawn_blocking(move || -> crate::instance::content::error::Result<_> {
        let snapshot = inventory_scope(&root, ContentKind::Mod)?;
        let mut entries: Vec<_> = snapshot
            .entries
            .into_iter()
            .map(|entry| {
                let metadata = std::fs::symlink_metadata(root.join(&entry.relative_path))?;
                Ok((entry.name, metadata.len(), metadata.modified().ok()))
            })
            .collect::<std::io::Result<_>>()?;
        entries.sort_by(|left, right| left.0.cmp(&right.0));
        let mut digest = sha2::Sha256::new();
        for (name, size, modified) in entries {
            digest.update(name.as_encoded_bytes());
            digest.update(size.to_le_bytes());
            digest.update(format!("{modified:?}").as_bytes());
        }
        Ok(format!("{:x}", digest.finalize()))
    })
    .await
}
