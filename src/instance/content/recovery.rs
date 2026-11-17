//! Восстановление add-only установки идёт вперёд из проверенного CAS.
//! Чужие файлы не удаляются: несовпадение hash останавливает восстановление.
use super::{
    journal,
    storage::{self, PendingPublication},
};
use crate::{
    data::{db::Database, layout::LauncherPaths},
    instance::content::{
        error::{ContentError, Result},
        store::ObjectStore,
    },
    instance::{model::InstanceId, service::InstanceService},
    utils::{
        cancellation::Cancellation,
        files::{self, placement},
    },
};
use std::{fs, io};

pub(super) struct RecoveryFailure {
    pub operation: String,
    pub instance: InstanceId,
    pub error: ContentError,
}

pub(super) async fn recover(
    db: &Database,
    paths: &LauncherPaths,
    instances: &InstanceService,
    filter: Option<InstanceId>,
) -> Result<Vec<RecoveryFailure>> {
    let store = ObjectStore::new(paths.root().join("content-store"));
    let mut failures = Vec::new();
    let mut blocked_instance = None;
    for pending in storage::pending(db, filter).await? {
        if blocked_instance == Some(pending.instance_id) {
            continue;
        }
        if let Err(error) = recover_one(db, paths, instances, &store, &pending).await {
            blocked_instance = Some(pending.instance_id);
            failures.push(RecoveryFailure {
                operation: pending.id,
                instance: pending.instance_id,
                error,
            });
        }
    }
    Ok(failures)
}

async fn recover_one(
    db: &Database,
    paths: &LauncherPaths,
    instances: &InstanceService,
    store: &ObjectStore,
    pending: &PendingPublication,
) -> Result<()> {
    let instance = pending.instance_id;
    let id = uuid::Uuid::parse_str(&pending.id)
        .ok()
        .filter(|id| id.to_string() == pending.id)
        .ok_or_else(|| ContentError::InvalidJournalId(pending.id.clone()))?;
    let mut expected_files = journal::decode(&pending.recovery_json)?;
    let leased_objects = storage::leased_objects(db, id).await?;
    // Validate the full file list before publishing any entry.
    let mut names = std::collections::HashSet::new();
    for file in &expected_files {
        files::portable_name::validate_file_name(&file.name)?;
        store.path_for(&file.sha256)?;
        if !names.insert(file.name.to_lowercase())
            || !leased_objects
                .get(&file.object_id)
                .is_some_and(|hash| hash.eq_ignore_ascii_case(&file.sha256))
            || !matches!(file.provider.as_str(), "modrinth" | "curseforge")
        {
            return Err(ContentError::Invalid(
                "invalid file entry in content journal".into(),
            ));
        }
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
    for file in &mut expected_files {
        let path = files::child(&mods, std::path::Path::new(&file.name))?;
        match fs::symlink_metadata(&path) {
            Ok(metadata) => {
                let matches = if metadata.is_file() {
                    let target = path.clone();
                    let expected = file.sha256.clone();
                    async_std::task::spawn_blocking(move || {
                        files::sha256_file(&target)
                            .map(|actual| actual.eq_ignore_ascii_case(&expected))
                    })
                    .await?
                } else {
                    false
                };
                if !matches {
                    return Err(ContentError::RecoveryConflict {
                        name: file.name.clone(),
                    });
                }
                // Matching bytes do not tell us how this entry was placed.
                file.placement = "external".into();
                continue;
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        let object = store.path_for(&file.sha256)?;
        let expected = file.sha256.clone();
        let staged = stage.path().join(&file.name);
        async_std::task::spawn_blocking(move || -> Result<()> {
            placement::copy_new(&object, &staged, &Cancellation::default())?;
            if !files::sha256_file(&staged)?.eq_ignore_ascii_case(&expected) {
                return Err(ContentError::Invalid(
                    "Объект CAS повреждён; восстановление остановлено".into(),
                ));
            }
            files::publish::publish_new(staged, path)?;
            Ok(())
        })
        .await?;
        file.placement = "copy".into();
    }
    storage::complete(db, id, instance, &expected_files).await?;
    Ok(())
}
