//! Восстановление add-only установки идёт вперёд из проверенного CAS.
//! Чужие файлы не удаляются: несовпадение hash останавливает восстановление.
use super::storage;
use crate::{
    data::{db::Database, layout::LauncherPaths},
    instance::content::{
        error::{ContentError, Result},
        store::ObjectStore,
    },
    instance::{model::InstanceId, service::InstanceService},
    utils::{
        cancellation::Cancellation,
        files::{
            self,
            placement::{self, CopyFallback},
        },
    },
};
use std::{fs, sync::Arc};

pub(crate) async fn recover(
    db: &Database,
    paths: &LauncherPaths,
    instances: &Arc<InstanceService>,
    filter: Option<InstanceId>,
) -> Result<()> {
    let store = Arc::new(ObjectStore::new(paths.root().join("content-store")));
    for (operation, instance, expected_files) in storage::pending(db).await? {
        if filter.is_some_and(|id| id != instance) {
            continue;
        }
        let record = instances
            .get_registry()
            .get(instance)
            .await
            .map_err(|error| ContentError::Invalid(error.to_string()))?;
        let root = files::child(paths.instances(), std::path::Path::new(&record.dir))?;
        let mods = files::child(&root, std::path::Path::new("mods"))?;
        fs::create_dir_all(&mods)?;
        let stage = tempfile::Builder::new()
            .prefix(".sonata-recovery-")
            .tempdir_in(&root)?;
        for file in &expected_files {
            files::portable_name::validate_file_name(&file.name)?;
            let path = files::child(&mods, std::path::Path::new(&file.name))?;
            if fs::symlink_metadata(&path).is_ok() {
                let target = path.clone();
                let expected = file.sha256.clone();
                let matches = async_std::task::spawn_blocking(move || {
                    files::sha256_file(&target).map(|actual| actual == expected)
                })
                .await?;
                if !matches {
                    return Err(ContentError::Invalid(format!(
                        "RecoveryConflict: {} изменён после прерванной установки",
                        file.name
                    )));
                }
                continue;
            }
            let object = store.path_for(&file.sha256)?;
            let expected = file.sha256.clone();
            let staged = stage.path().join(&file.name);
            async_std::task::spawn_blocking(move || -> Result<()> {
                if files::sha256_file(&object)? != expected {
                    return Err(ContentError::Invalid(
                        "Объект CAS повреждён; восстановление остановлено".into(),
                    ));
                }
                placement::place_new(
                    &object,
                    &staged,
                    CopyFallback::AllowedAfterLinkFailure,
                    &Cancellation::default(),
                )?;
                files::publish::publish_new(staged, path)?;
                Ok(())
            })
            .await?;
        }
        let id = uuid::Uuid::parse_str(&operation)
            .map_err(|_| ContentError::Invalid("Повреждён id журнала контента".into()))?;
        storage::complete(db, id, instance, &expected_files).await?;
    }
    Ok(())
}
