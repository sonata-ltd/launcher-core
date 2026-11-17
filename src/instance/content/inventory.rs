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

/// Hashes the contents and identity of every mod archive considered by a plan.
pub(super) async fn snapshot(
    root: PathBuf,
) -> crate::instance::content::error::Result<ModSnapshot> {
    async_std::task::spawn_blocking(move || {
        use sha2::{Digest, Sha256};
        let mut inventory = inventory_scope(&root, ContentKind::Mod)?;
        inventory
            .entries
            .sort_by(|left, right| left.name.cmp(&right.name));
        let mut digest = Sha256::new();
        let mut archives = std::collections::HashMap::new();
        let mut observed = std::collections::HashSet::new();
        for entry in inventory.entries {
            let name = entry.name.as_encoded_bytes();
            digest.update((name.len() as u64).to_le_bytes());
            digest.update(name);
            let path = root.join(&entry.relative_path);
            observed.insert(entry.relative_path.clone());
            match entry.entry_type {
                ObservedType::Archive(stamp) => {
                    let sha256 = crate::utils::files::sha256_file(&path)?;
                    let after = fs::symlink_metadata(&path)?;
                    if !after.is_file() || stamp != FileStamp::from_metadata(&after) {
                        return Err(io::Error::other("mod changed while planning").into());
                    }
                    digest.update([0]);
                    digest.update(sha256.as_bytes());
                    archives.insert(entry.relative_path, sha256);
                }
                ObservedType::SymbolicLink => {
                    digest.update([1]);
                    let target = fs::read_link(&path)?;
                    let bytes = target.as_os_str().as_encoded_bytes();
                    digest.update((bytes.len() as u64).to_le_bytes());
                    digest.update(bytes);
                }
                ObservedType::LocalDirectory => digest.update([2]),
            };
        }

        Ok(ModSnapshot {
            signature: format!("{:x}", digest.finalize()),
            archives,
            observed,
        })
    })
    .await
}

pub(super) struct ModSnapshot {
    pub signature: String,
    pub archives: std::collections::HashMap<PathBuf, String>,
    pub observed: std::collections::HashSet<PathBuf>,
}

pub(super) async fn signature(root: PathBuf) -> crate::instance::content::error::Result<String> {
    Ok(snapshot(root).await?.signature)
}
