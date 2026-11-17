//! Исполнение готового плана. Никакого выбора релизов или wire-DTO API.
use super::{journal::PublishedFile, storage};
use crate::{
    data::db::Database,
    instance::content::{
        catalog::types::CatalogRelease,
        error::{ContentError, Result},
        model::Provider,
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

pub(crate) async fn execute(
    db: Arc<Database>,
    store: Arc<ObjectStore>,
    instance: InstanceId,
    root: &Path,
    id: Uuid,
    releases: &[CatalogRelease],
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
    for (index, release) in releases.iter().enumerate() {
        cancel.check()?;
        let file = &release.file;
        files::portable_name::validate_file_name(&file.file_name)?;
        if file.sha1.is_none() && file.sha512.is_none() {
            return Err(ContentError::Invalid(
                "Провайдер не предоставил проверяемый хеш".into(),
            ));
        }
        let url = surf::Url::parse(&file.url)
            .map_err(|_| ContentError::Invalid("Invalid download URL".into()))?;
        if url.scheme() != "https" || !url.username().is_empty() || url.password().is_some() {
            return Err(ContentError::Invalid(
                "Загрузка контента требует HTTPS без credentials в URL".into(),
            ));
        }
        if file.size_bytes > 2 * 1024 * 1024 * 1024 {
            return Err(ContentError::Invalid(
                "Этот установщик поддерживает архивы до 2 GiB".into(),
            ));
        }
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
        let object_id = crate::instance::content::store::storage::register(&db, &object).await?;
        let target = stage.path().join(&file.file_name);
        let source = object.path.clone();
        let token = common_cancel.clone();
        async_std::task::spawn_blocking(move || placement::copy_new(&source, &target, &token))
            .await?;
        let provider = match release.key.project.provider {
            Provider::Modrinth => "modrinth",
            Provider::CurseForge => "curseforge",
        };
        prepared.push(PublishedFile {
            name: file.file_name.clone(),
            sha256: object.digests.sha256,
            object_id,
            provider: provider.into(),
            project_id: release.key.project.project_id.clone(),
            release_id: release.key.release_id.clone(),
            placement: "copy".into(),
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
    for file in &prepared {
        let source = stage.path().join(&file.name);
        let target = mods.join(&file.name);
        async_std::task::spawn_blocking(move || files::publish::publish_new(source, target))
            .await?;
    }
    storage::complete(&db, id, instance, &prepared).await?;
    Ok(())
}
