//! Восстановление add-only установки идёт вперёд из проверенного CAS.
//! Чужие файлы не удаляются: несовпадение hash останавливает восстановление.
use super::{
    journal::{self, Placement},
    storage::{self, PendingPublication},
};
use crate::{
    data::{db::Database, layout::LauncherPaths},
    instance::content::{
        error::{ContentError, Result},
        store::ObjectStore,
    },
    instance::{model::InstanceId, registry::InstanceRegistry},
    utils::{
        cancellation::Cancellation,
        files::{
            self, digest::copy_and_digest, identity::FileIdentity, owned_directory::OwnedDirectory,
        },
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
    instances: &InstanceRegistry,
    filter: Option<InstanceId>,
) -> Result<Vec<RecoveryFailure>> {
    let store = ObjectStore::beneath(paths.root(), std::path::Path::new("content-store"))?
        .access(&Cancellation::default())
        .await?;
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
    instances: &InstanceRegistry,
    store: &crate::utils::object_store::StoreAccess,
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
        {
            return Err(ContentError::Invalid(
                "invalid file entry in content journal".into(),
            ));
        }
    }
    let record = instances.get(instance).await.map_err(ContentError::from)?;
    let root = files::child(paths.instances(), std::path::Path::new(&record.dir))?;
    let instance_directory =
        crate::data::ownership::ensure_instance_directory(db.layout_lease.as_deref(), &root)?;
    let root = instance_directory.path();
    let mods = files::child(root, std::path::Path::new("mods"))?;
    let mods_directory = instance_directory.create_child(std::ffi::OsStr::new("mods"))?;
    let stage = OwnedDirectory::create_in(&instance_directory, super::staging::RECOVERY_PREFIX)?;
    let staged_directory = stage
        .directory()
        .create_private_child(std::ffi::OsStr::new("files"))?;
    for file in &mut expected_files {
        let path = files::child(&mods, std::path::Path::new(&file.name))?;
        match fs::symlink_metadata(&path) {
            Ok(metadata) => {
                let relative = std::path::Path::new("mods").join(&file.name);
                storage::guard_binding_aliases(db, instance, root, &relative, None).await?;
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
                file.placement = Placement::External;
                continue;
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        if file.placement == Placement::External {
            return Err(ContentError::RecoveryConflict {
                name: file.name.clone(),
            });
        }
        let object = store.path_for(&file.sha256)?;
        let expected = file.sha256.clone();
        let reader = store.clone();
        let from = staged_directory.clone();
        let to = mods_directory.clone();
        let name = file.name.clone();
        async_std::task::spawn_blocking(move || -> Result<()> {
            let _access = reader;
            let name = std::ffi::OsStr::new(&name);
            let mut staged = from.create_file(name)?;
            let bytes = copy_and_digest(&object, &mut staged, &Cancellation::default())?;
            if !bytes.sha256.eq_ignore_ascii_case(&expected) {
                return Err(ContentError::Invalid(
                    "content byte object is corrupt".into(),
                ));
            }
            staged.sync_all()?;
            from.verify_file(name, FileIdentity::from_handle(&staged)?)?;
            from.publish(name, &to, name)?;
            Ok(())
        })
        .await?;
        file.placement = Placement::Copy;
    }
    #[cfg(test)]
    super::install::checkpoints::wait(id, "recovering").await;
    mods_directory.verify_location()?;
    storage::complete(db, id, instance, &expected_files).await?;
    Ok(())
}
