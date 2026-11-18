//! Исполнение готового плана. Никакого выбора релизов или wire-DTO API.
use super::{
    journal::{Placement, PublishedFile},
    storage,
};
use crate::{
    data::db::Database,
    instance::content::{
        catalog::types::CatalogRelease,
        error::{ContentError, Result},
        store::ObjectStore,
    },
    instance::model::InstanceId,
    utils::{
        download::transfer::{self, Cancel},
        files::{self, digest::digest_file, placement},
    },
};
use std::{fs, path::Path, sync::Arc};
use uuid::Uuid;

#[cfg(test)]
pub(super) mod checkpoints;

pub(super) struct PlannedRelease {
    pub release: CatalogRelease,
    pub adopt_existing: bool,
}

pub(super) async fn execute(
    db: Arc<Database>,
    store: Arc<ObjectStore>,
    instance: InstanceId,
    root: &Path,
    id: Uuid,
    releases: &[PlannedRelease],
    expected_signature: &str,
    cancel: &Cancel,
    progress: impl Fn(usize, bool),
) -> Result<()> {
    cancel.check()?;
    let access = store.access(cancel).await;
    cancel.check()?;
    let access = access?;
    let identity = access.root_key()?;
    storage::begin_preparation(&db, id, instance, &identity).await?;
    let result = execute_preparing(
        db.clone(),
        access.clone(),
        instance,
        root,
        id,
        releases,
        expected_signature,
        cancel,
        progress,
    )
    .await;
    // On error/cancellation only preparing may be discarded. Once publishing,
    // the durable journal and leases belong to recovery, not the worker future.
    if let Err(error) = storage::discard_preparation(&db, id).await {
        tracing::warn!(%id, %error, "could not release preparation leases");
    }
    result
}

async fn execute_preparing(
    db: Arc<Database>,
    store: crate::utils::object_store::StoreAccess,
    instance: InstanceId,
    root: &Path,
    id: Uuid,
    releases: &[PlannedRelease],
    expected_signature: &str,
    cancel: &Cancel,
    progress: impl Fn(usize, bool),
) -> Result<()> {
    let mods = files::child(root, Path::new("mods"))?;
    fs::create_dir_all(&mods)?;
    // Временные записи лежат вне mods и на том же томе, что назначения.
    let stage = super::staging::ContentStaging::create(root)?;
    let mut prepared = Vec::new();
    for (index, action) in releases.iter().enumerate() {
        let release = &action.release;
        cancel.check()?;
        let file = &release.file;
        file.validate()?;
        let (object_id, sha256, placement) = if action.adopt_existing {
            let target = files::child(&mods, Path::new(&file.file_name))?;
            let expected = file.clone();
            let token = cancel.clone();
            let hashes = async_std::task::spawn_blocking(move || -> Result<_> {
                let (hashes, _) = digest_file(&target, &token, |_| {})?;
                if !expected.matches(&hashes) {
                    return Err(ContentError::RecoveryConflict {
                        name: expected.file_name,
                    });
                }
                Ok(hashes)
            })
            .await?;
            let object_id = super::store::storage::register_leased(&db, &hashes, false, id).await?;
            (object_id, hashes.sha256, Placement::External)
        } else {
            let cached_hash = crate::instance::content::store::storage::cached_hash(
                &db,
                file.sha1.as_deref(),
                file.sha512.as_deref(),
                file.size_bytes,
                id,
            )
            .await?;
            let cached_object = if let Some(hash) = cached_hash {
                let path = store.path_for(&hash)?;
                if path.try_exists()? {
                    let token = cancel.clone();
                    let expected = file.clone();
                    let reader = store.clone();
                    Some(
                        async_std::task::spawn_blocking(move || -> Result<_> {
                            let _access = reader;
                            let (hashes, _) = digest_file(&path, &token, |_| {})?;
                            if hashes.sha256 != hash || !expected.matches(&hashes) {
                                return Err(ContentError::Invalid(
                                    "shared byte object is corrupt".into(),
                                ));
                            }
                            Ok(crate::instance::content::store::StoredObject {
                                digests: hashes,
                                path,
                            })
                        })
                        .await?,
                    )
                } else {
                    None
                }
            } else {
                None
            };
            let (object, object_id) = if let Some(object) = cached_object {
                let object_id =
                    super::store::storage::register_leased(&db, &object.digests, true, id).await?;
                (object, object_id)
            } else {
                let download = stage.downloads().join(format!("{index}.jar"));
                transfer::download(&file.url, &download, file.sha1.as_deref(), cancel).await?;
                let expected = file.clone();
                import_and_register(&db, store.clone(), download, id, cancel, move |hashes| {
                    if !expected.matches(hashes) {
                        return Err(ContentError::Invalid(
                            "downloaded file size or digest differs from provider metadata".into(),
                        ));
                    }
                    Ok(())
                })
                .await?
            };
            let target = stage.files().join(&file.file_name);
            let source = object.path.clone();
            let token = cancel.clone();
            let reader = store.clone();
            async_std::task::spawn_blocking(move || {
                let _access = reader;
                placement::copy_new(&source, &target, &token)
            })
            .await?;
            (object_id, object.digests.sha256, Placement::Copy)
        };
        prepared.push(PublishedFile {
            name: file.file_name.clone(),
            sha256,
            object_id,
            provider: release.key.project.provider,
            project_id: release.key.project.project_id.clone(),
            release_id: release.key.release_id.clone(),
            placement,
            dependencies: Some(release.dependencies.clone()),
        });
        progress(index + 1, false);
    }
    cancel.check()?;
    if super::inventory::signature(root.to_owned()).await? != expected_signature {
        return Err(ContentError::StalePlan(
            super::error::StalePlanReason::InventoryChanged,
        ));
    }
    storage::prepare(&db, id, instance, &prepared).await?;
    progress(releases.len(), true);
    // После сохранения намерения commit не прерывается Cancel. Все назначения
    // новые; persist_noclobber не перезапишет появившийся файл пользователя.
    for file in prepared
        .iter()
        .filter(|file| file.placement != Placement::External)
    {
        let source = stage.files().join(&file.name);
        let target = mods.join(&file.name);
        let publisher = store.clone();
        async_std::task::spawn_blocking(move || {
            let _access = publisher;
            files::publish::publish_new(source, target)
        })
        .await?;
    }
    #[cfg(test)]
    checkpoints::wait(id, "completing").await;
    storage::complete(&db, id, instance, &prepared).await?;
    Ok(())
}

/// Keep admission across the filesystem publication -> SQL registration gap.
/// The enclosing installation retains its own clone through placement/completion.
pub(super) async fn import_and_register(
    db: &Database,
    store: crate::utils::object_store::StoreAccess,
    source: std::path::PathBuf,
    operation: Uuid,
    cancel: &Cancel,
    validate: impl FnOnce(&files::digest::FileDigests) -> Result<()> + Send + 'static,
) -> Result<(super::store::StoredObject, i64)> {
    let worker = store.clone();
    let token = cancel.clone();
    let object =
        async_std::task::spawn_blocking(move || worker.import_checked(&source, &token, validate))
            .await?;
    #[cfg(test)]
    checkpoints::wait(operation, "imported").await;
    let object_id =
        super::store::storage::register_leased(db, &object.digests, true, operation).await?;
    Ok((object, object_id))
}
