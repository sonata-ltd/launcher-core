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
        cancellation::Cancellation,
        download::transfer::{self, Cancel},
        files::{self, digest::digest_file, placement},
    },
};
use std::{fs, path::Path, sync::Arc};
use uuid::Uuid;

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
    let mods = files::child(root, Path::new("mods"))?;
    fs::create_dir_all(&mods)?;
    // Временные записи лежат вне mods и на том же томе, что назначения.
    let stage = tempfile::Builder::new()
        .prefix(".sonata-content-")
        .tempdir_in(root)?;
    let download_stage = tempfile::tempdir()?;
    let mut prepared = Vec::new();
    let common_cancel = Cancellation::from_flag(cancel.0.clone());
    for (index, action) in releases.iter().enumerate() {
        let release = &action.release;
        cancel.check()?;
        let file = &release.file;
        file.validate()?;
        let (object_id, sha256, placement) = if action.adopt_existing {
            let target = files::child(&mods, Path::new(&file.file_name))?;
            let expected = file.clone();
            let token = common_cancel.clone();
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
            let object_id = super::store::storage::register_external(&db, &hashes).await?;
            (object_id, hashes.sha256, Placement::External)
        } else {
            let cached_hash = crate::instance::content::store::storage::cached_hash(
                &db,
                file.sha1.as_deref(),
                file.sha512.as_deref(),
                file.size_bytes,
            )
            .await?;
            let cached_object = if let Some(hash) = cached_hash {
                let path = store.path_for(&hash)?;
                if path.try_exists()? {
                    let token = common_cancel.clone();
                    let expected = file.clone();
                    Some(
                        async_std::task::spawn_blocking(move || -> Result<_> {
                            let (hashes, _) = digest_file(&path, &token, |_| {})?;
                            if hashes.sha256 != hash || !expected.matches(&hashes) {
                                return Err(ContentError::Invalid(
                                    "Объект общего хранилища повреждён".into(),
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
            let object = if let Some(object) = cached_object {
                object
            } else {
                let download = download_stage.path().join(format!("{index}.jar"));
                transfer::download(&file.url, &download, file.sha1.as_deref(), cancel).await?;
                let source = download.clone();
                let token = common_cancel.clone();
                let expected = file.clone();
                let store_copy = store.clone();
                let object = async_std::task::spawn_blocking(move || -> Result<_> {
                    let (hashes, _) = digest_file(&source, &token, |_| {})?;
                    if !expected.matches(&hashes) {
                        return Err(ContentError::Invalid(
                            "Размер или SHA-512 скачанного файла не совпадает с API".into(),
                        ));
                    }
                    Ok(store_copy.import_file(&source, Some(&hashes.sha256), &token)?)
                })
                .await?;
                object
            };
            let object_id =
                crate::instance::content::store::storage::register(&db, &object).await?;
            let target = stage.path().join(&file.file_name);
            let source = object.path.clone();
            let token = common_cancel.clone();
            async_std::task::spawn_blocking(move || placement::copy_new(&source, &target, &token))
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
        return Err(ContentError::Invalid(
            "Состав mods изменился во время загрузки; постройте план заново".into(),
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
        let source = stage.path().join(&file.name);
        let target = mods.join(&file.name);
        async_std::task::spawn_blocking(move || files::publish::publish_new(source, target))
            .await?;
    }
    storage::complete(&db, id, instance, &prepared).await?;
    Ok(())
}
