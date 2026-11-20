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
        files::{
            self,
            digest::{copy_and_digest, digest_file},
        },
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
    let instance_directory =
        crate::data::ownership::ensure_instance_directory(db.layout_lease.as_deref(), root)?;
    let root = instance_directory.path();
    let mods = files::child(root, Path::new("mods"))?;
    let mods_directory = instance_directory.create_child(std::ffi::OsStr::new("mods"))?;
    // Временные записи лежат вне mods и на том же томе, что назначения.
    let stage = super::staging::ContentStaging::create(root)?;
    let stage_files = stage.files();
    let staged_directory = instance_directory.open_relative(
        stage_files
            .strip_prefix(root)
            .map_err(std::io::Error::other)?,
    )?;
    let mut prepared = Vec::new();
    for (index, action) in releases.iter().enumerate() {
        let release = &action.release;
        cancel.check()?;
        let file = &release.file;
        file.validate()?;
        let (object_id, sha256, placement) = if action.adopt_existing {
            let target = files::child(&mods, Path::new(&file.file_name))?;
            let relative = Path::new("mods").join(&file.file_name);
            storage::guard_binding_aliases(&db, instance, root, &relative, None).await?;
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
            #[cfg(test)]
            checkpoints::wait(id, "before-content-copy").await;
            let target = stage.files().join(&file.file_name);
            let source = object.path.clone();
            let token = cancel.clone();
            let reader = store.clone();
            let expected = object.digests.clone();
            let name = file.file_name.clone();
            async_std::task::spawn_blocking(move || -> Result<()> {
                let _access = reader;
                let mut staged = fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(target)?;
                let actual = copy_and_digest(&source, &mut staged, &token)?;
                if actual.sha256 != expected.sha256 || actual.size_bytes != expected.size_bytes {
                    return Err(ContentError::StagedDigestMismatch { name });
                }
                staged.sync_all()?;
                Ok(())
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
        let publisher = store.clone();
        let from = staged_directory.clone();
        let to = mods_directory.clone();
        let expected = file.sha256.clone();
        let name = file.name.clone();
        async_std::task::spawn_blocking(move || -> Result<()> {
            let _access = publisher;
            if !fs::symlink_metadata(&source)?.is_file() || files::sha256_file(&source)? != expected
            {
                return Err(ContentError::StagedDigestMismatch { name });
            }
            let name = std::ffi::OsStr::new(&name);
            from.publish(name, &to, name)?;
            Ok(())
        })
        .await?;
    }
    #[cfg(test)]
    checkpoints::wait(id, "completing").await;
    mods_directory.verify_location()?;
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
