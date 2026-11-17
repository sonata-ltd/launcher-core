use std::path::{Path, PathBuf};

use crate::instance::{
    content::inventory::{self, ContentKind, ObservedType},
    error::{InstanceError, Result},
    model::{InstanceId, InstanceMod},
};

pub async fn resolve_mods(instance_id: InstanceId, path: PathBuf) -> Result<Vec<InstanceMod>> {
    let inventory = async_std::task::spawn_blocking(move || {
        inventory::inventory_directory(&path, Path::new("mods"), ContentKind::Mod)
            .map_err(|source| InstanceError::ModInventory { path, source })
    })
    .await?;

    Ok(inventory
        .entries
        .into_iter()
        .filter(|entry| matches!(entry.entry_type, ObservedType::Archive(_)))
        .map(|entry| InstanceMod {
            instance_id,
            name: entry.name,
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[async_std::test]
    async fn returns_only_mod_archives_and_reports_unreadable_layout() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("mods");
        let id = 1.into();
        assert!(resolve_mods(id, dir.clone()).await.unwrap().is_empty());
        std::fs::create_dir(&dir).unwrap();
        for name in ["first.jar", "second.jar.disabled", "readme.txt"] {
            std::fs::write(dir.join(name), b"fixture").unwrap();
        }
        let mut names: Vec<_> = resolve_mods(id, dir)
            .await
            .unwrap()
            .into_iter()
            .map(|record| record.name)
            .collect();
        names.sort();
        assert_eq!(names, ["first.jar", "second.jar.disabled"]);
        assert!(matches!(
            resolve_mods(id, root.path().join("mods/readme.txt")).await,
            Err(InstanceError::ModInventory { .. })
        ));
    }
}
