//! Сценарии конкретного экземпляра: каталог → план → фоновая установка.
//! HTTP и нормализация находятся в crate::instance::content::catalog, SQL — в storage.
use std::{
    collections::HashMap,
    fs::OpenOptions,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use super::{api::*, install, recovery, storage};
use crate::{
    data::{db::Database, layout::LauncherPaths},
    instance::content::{
        catalog::{types::*, ContentCatalog},
        error::{ContentError, Result},
        model::{ProjectKey, Provider},
        resolve::catalog_plan,
        store::ObjectStore,
    },
    instance::{
        activity::InstanceActivity, model::InstanceId, registry::InstanceRegistry,
        service::InstanceService,
    },
    utils::{download::transfer::Cancel, files},
};
use uuid::Uuid;

struct StoredPlan {
    preview: InstallPreview,
    releases: Vec<install::PlannedRelease>,
    signature: String,
    compatibility: Compatibility,
    created: Instant,
}
struct Job {
    snapshot: ContentJobSnapshot,
    cancel: Cancel,
    created: Instant,
}

/// Add-only content publication with forward recovery after a process crash.
///
/// `Completed` is not a promise of power-loss durability: filesystem directory
/// entries and SQLite are not committed as one transaction. See the publication
/// contract in `docs/publication-contract.md`. External writers may cause a
/// recovery conflict; recovery never replaces their differing bytes.
pub struct InstanceContentService {
    operations: Arc<crate::data::registry::operation::OperationRegistry>,
    catalog: ContentCatalog,
    instances: Arc<InstanceRegistry>,
    activity: Arc<InstanceActivity>,
    mod_index: Arc<super::index::ModIndex>,
    paths: Arc<LauncherPaths>,
    db: Arc<Database>,
    store: Arc<ObjectStore>,
    plans: Mutex<HashMap<Uuid, StoredPlan>>,
    jobs: Mutex<HashMap<Uuid, Job>>,
    // OS-lock защищает от второго обновлённого процесса с тем же корнем.
    // In-memory барьер InstanceService защищает launch и install внутри процесса.
    _root_lock: crate::utils::object_store::StoreLock,
    _store_lock: crate::utils::object_store::StoreLock,
}

impl InstanceContentService {
    pub async fn new(
        db: Arc<Database>,
        paths: Arc<LauncherPaths>,
        instances: Arc<InstanceService>,
        curseforge_key: Option<String>,
        operations: Arc<crate::data::registry::operation::OperationRegistry>,
    ) -> Result<Self> {
        let root_lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(paths.root().join(".launcher-content.lock"))?;
        let root_lock =
            crate::utils::object_store::StoreLock::exclusive(root_lock).map_err(|_| {
                ContentError::Invalid("Другой процесс лаунчера уже использует этот корень".into())
            })?;
        let registry = instances.get_registry();
        let activity = instances.activity();
        let store = Arc::new(ObjectStore::new(paths.root().join("content-store")));
        let store_lock = store.lock_exclusive()?;
        let maintenance = store.try_maintenance()?;
        let cleanup_db = db.clone();
        async_std::task::spawn(async move {
            // Do not admit an uncatalogued new writer to a previously complete
            // store. Unknown/ephemeral DBs durably disable GC inside attach.
            super::retention::attach(&cleanup_db, &maintenance).await?;
            #[cfg(test)]
            if let Some(operation) = storage::preparation_checkpoint(&cleanup_db).await? {
                super::install::checkpoints::wait(operation, "cleanup").await;
            }
            storage::clear_abandoned_preparations(&cleanup_db, &maintenance).await
        })
        .await?;
        let scratch_store = store.clone();
        async_std::task::spawn_blocking(move || match scratch_store.sweep_staging() {
            Ok(removed) if removed > 0 => {
                tracing::info!(removed, "cleaned up abandoned content imports")
            }
            Ok(_) => {}
            Err(error) => tracing::warn!(%error, "could not sweep content import staging"),
        })
        .await;
        super::staging::sweep_stale(paths.instances().clone()).await;
        match recovery::recover(&db, &paths, &registry, None).await {
            Ok(failures) => {
                for failure in failures {
                    tracing::error!(
                        instance = %failure.instance,
                        operation = %failure.operation,
                        error = %failure.error,
                        "content recovery requires attention"
                    );
                }
            }
            Err(error) => tracing::error!(%error, "could not load pending content publications"),
        }
        storage::clear_completed(&db, None).await?;
        if let Err(error) =
            super::store::legacy::detach_registered_links(&db, &paths, &activity).await
        {
            tracing::warn!(%error, "could not migrate registered hardlinks");
        }
        let collection_store = store.clone();
        async_std::task::spawn(async move {
            match super::retention::collect(
                collection_store,
                std::time::Duration::from_secs(7 * 24 * 60 * 60),
            )
            .await
            {
                Ok(removed) if removed > 0 => {
                    tracing::info!(removed, "collected unreferenced content objects")
                }
                Ok(_) => {}
                Err(error) => tracing::debug!(%error, "content collection deferred"),
            }
        })
        .await;
        Ok(Self {
            operations,
            catalog: ContentCatalog::new(curseforge_key),
            instances: registry,
            mod_index: instances.mod_index(),
            activity,
            paths: paths.clone(),
            db,
            store,
            plans: Mutex::new(HashMap::new()),
            jobs: Mutex::new(HashMap::new()),
            _root_lock: root_lock,
            _store_lock: store_lock,
        })
    }

    async fn context(&self, instance: InstanceId) -> Result<(PathBuf, Compatibility)> {
        let record = self
            .instances
            .get(instance)
            .await
            .map_err(|error| ContentError::Invalid(error.to_string()))?;
        if matches!(
            record.loader,
            crate::modloader::model::LoaderKind::Vanilla
                | crate::modloader::model::LoaderKind::Cleanroom
        ) {
            return Err(ContentError::Invalid(
                "Для этого экземпляра автоматическая установка модов пока не поддерживается".into(),
            ));
        }
        let root = files::child(self.paths.instances(), std::path::Path::new(&record.dir))?;
        Ok((
            root,
            Compatibility {
                minecraft: record.version.clone(),
                loader: record.loader.as_str().into(),
            },
        ))
    }

    pub async fn search(
        &self,
        instance: InstanceId,
        provider: Provider,
        query: &str,
        offset: u32,
    ) -> Result<SearchPage> {
        let (_, compatibility) = self.context(instance).await?;
        self.catalog
            .search(provider, query, offset, &compatibility)
            .await
    }

    pub async fn plan_install(
        &self,
        instance: InstanceId,
        roots: Vec<ProjectKey>,
    ) -> Result<InstallPreview> {
        storage::ensure_no_pending(&self.db, instance).await?;
        let (root, compatibility) = self.context(instance).await?;
        let snapshot = super::inventory::snapshot(root.clone()).await?;
        let registered = storage::registered_mods(&self.db, instance).await?;
        let mut installed = HashMap::new();
        let mut already_present = Vec::new();
        for entry in &registered {
            let Some(path) = entry.verified_path(&snapshot) else {
                continue;
            };
            if !path
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("jar"))
            {
                continue;
            }
            let project = entry.project();
            let candidate = super::model::Candidate {
                key: super::model::ReleaseKey {
                    project: project.clone(),
                    release_id: entry.release_id.clone(),
                },
                dependencies: entry.dependencies()?,
            };
            if let Some(previous) = installed.insert(project.clone(), candidate) {
                if previous.key.release_id != entry.release_id {
                    return Err(ContentError::Invalid(
                        "Несколько активных версий одного проекта".into(),
                    ));
                }
                continue;
            }
            if roots.contains(&project) {
                already_present.push(PreviewItem {
                    title: project.project_id.clone(),
                    file_name: path
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned(),
                    project,
                    is_root: true,
                    size_bytes: entry.size_bytes as u64,
                    already_installed: true,
                });
            }
        }
        let all_releases = async_std::future::timeout(
            Duration::from_secs(90),
            catalog_plan::resolve_with_installed(&self.catalog, &roots, &compatibility, installed),
        )
        .await
        .map_err(|_| ContentError::Transport("Подбор зависимостей превысил 90 секунд".into()))??;
        let mut names = std::collections::HashSet::new();
        let mut releases = Vec::new();
        let mut items = already_present;
        for release in all_releases {
            files::portable_name::validate_file_name(&release.file.file_name)?;
            // Переносимое консервативное ограничение для создаваемых имён.
            if !names.insert(release.file.file_name.to_lowercase()) {
                return Err(ContentError::Invalid(
                    "Разные проекты создают одинаковое имя файла".into(),
                ));
            }
            let target = root.join("mods").join(&release.file.file_name);
            let already_installed = if std::fs::symlink_metadata(&target).is_ok() {
                let file = release.file.clone();
                let matches = async_std::task::spawn_blocking(move || -> Result<bool> {
                    let (actual, _) = files::digest::digest_file(
                        &target,
                        &crate::utils::cancellation::Cancellation::default(),
                        |_| {},
                    )?;
                    Ok(file.matches(&actual))
                })
                .await?;
                if !matches {
                    return Err(ContentError::Invalid(format!(
                        "{} уже существует с другим содержимым. Перезапись отключена",
                        release.file.file_name
                    )));
                }
                true
            } else {
                false
            };
            let existing = registered
                .iter()
                .any(|entry| entry.matches(&release.key.project, &snapshot));
            if existing && !already_installed {
                return Err(ContentError::Invalid(
                    "Уже установлена другая версия проекта; обновление требует отдельного плана"
                        .into(),
                ));
            }
            items.push(PreviewItem {
                title: release.title.clone(),
                file_name: release.file.file_name.clone(),
                project: release.key.project.clone(),
                is_root: roots.contains(&release.key.project),
                size_bytes: release.file.size_bytes,
                already_installed,
            });
            if !already_installed
                || !registered.iter().any(|entry| {
                    entry.matches(&release.key.project, &snapshot)
                        && std::path::Path::new(&entry.relative_path)
                            == std::path::Path::new("mods").join(&release.file.file_name)
                })
            {
                releases.push(install::PlannedRelease {
                    release,
                    adopt_existing: already_installed,
                });
            }
        }
        if super::inventory::signature(root).await? != snapshot.signature {
            return Err(ContentError::Invalid(
                "Папка mods изменилась при подготовке плана".into(),
            ));
        }
        let signature = snapshot.signature;
        let preview = InstallPreview {
            id: Uuid::new_v4(),
            instance_id: instance,
            download_bytes: releases
                .iter()
                .filter(|action| !action.adopt_existing)
                .map(|action| action.release.file.size_bytes)
                .sum(),
            items,
        };
        let mut plans = self
            .plans
            .lock()
            .map_err(|_| ContentError::Invalid("Plan lock poisoned".into()))?;
        plans.retain(|_, plan| plan.created.elapsed() < Duration::from_secs(300));
        if plans.len() >= 32 {
            return Err(ContentError::Invalid(
                "Слишком много предпросмотров; повторите позднее".into(),
            ));
        }
        plans.insert(
            preview.id,
            StoredPlan {
                preview: preview.clone(),
                releases,
                signature,
                compatibility,
                created: Instant::now(),
            },
        );
        Ok(preview)
    }

    pub fn start_install(self: &Arc<Self>, plan_id: Uuid) -> Result<Uuid> {
        let plan = self
            .plans
            .lock()
            .map_err(|_| ContentError::Invalid("Plan lock poisoned".into()))?
            .remove(&plan_id)
            .ok_or_else(|| ContentError::Invalid("План устарел; обновите предпросмотр".into()))?;
        if plan.created.elapsed() >= Duration::from_secs(300) {
            return Err(ContentError::Invalid(
                "План устарел; обновите предпросмотр".into(),
            ));
        }
        if plan.releases.is_empty() {
            return Err(ContentError::Invalid(
                "Все выбранные файлы уже установлены".into(),
            ));
        }
        let instance = plan.preview.instance_id;
        let activity = self
            .activity
            .reserve_content_change(instance)
            .map_err(|error| ContentError::Invalid(error.to_string()))?;
        let id = Uuid::new_v4();
        use crate::data::registry::operation::message::stage::OperationStage;
        let operation = self.operations.begin(
            format!("instance.content.install:{}", instance.get()),
            vec![
                OperationStage::DownloadContent,
                OperationStage::PublishContent,
            ],
        );
        let operation_id = operation.id();
        let cancel = Cancel::default();
        {
            let mut jobs = self
                .jobs
                .lock()
                .map_err(|_| ContentError::Invalid("Job lock poisoned".into()))?;
            jobs.retain(|_, job| {
                job.snapshot.active() || job.created.elapsed() < Duration::from_secs(3600)
            });
            if jobs.len() >= 128 {
                return Err(ContentError::Invalid(
                    "История установок заполнена; перезапустите лаунчер".into(),
                ));
            }
            jobs.insert(
                id,
                Job {
                    snapshot: ContentJobSnapshot {
                        operation_id,
                        id,
                        instance_id: instance,
                        phase: ContentPhase::Downloading,
                        completed_files: 0,
                        total_files: plan.releases.len(),
                        error: None,
                    },
                    cancel: cancel.clone(),
                    created: Instant::now(),
                },
            );
        }
        let service = self.clone();
        async_std::task::spawn(async move {
            // Job владеет activity, а не GUI Task: закрытие страницы не даёт
            // запустить игру посередине установки и не теряет результат.
            let _activity = activity;
            use crate::data::registry::operation::message::status::Outcome;
            operation.start_stage_determinable(OperationStage::DownloadContent);
            let result = service.execute_plan(id, plan, &cancel, &operation).await;
            let mut error = result.err();
            if error.is_some() {
                match recovery::recover(
                    &service.db,
                    &service.paths,
                    &service.instances,
                    Some(instance),
                )
                .await
                {
                    Ok(failures) => {
                        if let Some(failure) = failures.into_iter().next() {
                            error = Some(failure.error);
                        } else if storage::operation_completed(&service.db, id)
                            .await
                            .unwrap_or(false)
                        {
                            error = None;
                        }
                    }
                    Err(recovery_error) => error = Some(recovery_error),
                }
            }
            if let Ok(mut jobs) = service.jobs.lock() {
                if let Some(job) = jobs.get_mut(&id) {
                    job.snapshot.phase = if error.is_none() {
                        ContentPhase::Completed
                    } else if cancel.check().is_err()
                        && job.snapshot.phase == ContentPhase::Downloading
                    {
                        ContentPhase::Cancelled
                    } else {
                        ContentPhase::Failed
                    };
                    job.snapshot.error = error.as_ref().map(|error| error.to_string());
                }
            }
            let cancelled = service
                .jobs
                .lock()
                .ok()
                .and_then(|jobs| {
                    jobs.get(&id)
                        .map(|job| job.snapshot.phase == ContentPhase::Cancelled)
                })
                .unwrap_or(false);
            let outcome = if error.is_none() {
                Outcome::Completed
            } else if cancelled {
                Outcome::Cancelled
            } else {
                Outcome::Failed
            };
            operation.finish(outcome);
            if error.is_none() {
                if let Err(cleanup_error) = storage::clear_completed(&service.db, Some(id)).await {
                    tracing::warn!(error = %cleanup_error, "completed content journal cleanup failed");
                }
            }
        });
        Ok(id)
    }

    async fn execute_plan(
        &self,
        id: Uuid,
        plan: StoredPlan,
        cancel: &Cancel,
        operation: &crate::data::registry::operation::handle::OperationHandle,
    ) -> Result<()> {
        use crate::data::registry::operation::message::{
            stage::{OperationStage, StageResult},
            status::Outcome,
        };
        let started = Instant::now();
        let publish_started = Mutex::new(Instant::now());
        let (root, compatibility) = self.context(plan.preview.instance_id).await?;
        // Context повторно получен, но установка не меняет версию/loader.
        // Под тем же activity lease другие сценарии не могут менять экземпляр.
        if compatibility != plan.compatibility {
            return Err(ContentError::Invalid(
                "Версия Minecraft или загрузчик изменились после предпросмотра".into(),
            ));
        }
        storage::ensure_no_pending(&self.db, plan.preview.instance_id).await?;
        let snapshot = super::inventory::snapshot(root.clone()).await?;
        if snapshot.signature != plan.signature {
            return Err(ContentError::Invalid(
                "Папка mods изменилась после предпросмотра; постройте план заново".into(),
            ));
        }
        storage::reconcile_registered_mods(&self.db, plan.preview.instance_id, &snapshot).await?;
        self.mod_index
            .watch(&self.db, &self.activity, plan.preview.instance_id, &root);
        let result = install::execute(
            self.db.clone(),
            self.store.clone(),
            plan.preview.instance_id,
            &root,
            id,
            &plan.releases,
            &plan.signature,
            cancel,
            |done, publishing| {
                if publishing {
                    operation.complete_stage(StageResult {
                        stage: OperationStage::DownloadContent,
                        status: Outcome::Completed,
                        duration_secs: started.elapsed().as_secs_f64(),
                        error: None,
                    });
                    if let Ok(mut started) = publish_started.lock() {
                        *started = Instant::now();
                    }
                    operation.start_stage_determinable(OperationStage::PublishContent);
                } else {
                    operation.update_stage(
                        OperationStage::DownloadContent,
                        done,
                        plan.releases.len(),
                        None,
                    );
                }
                if let Ok(mut jobs) = self.jobs.lock() {
                    if let Some(job) = jobs.get_mut(&id) {
                        job.snapshot.completed_files = done;
                        if publishing {
                            job.snapshot.phase = ContentPhase::Publishing;
                        }
                    }
                }
            },
        )
        .await;
        if result.is_ok() {
            operation.complete_stage(StageResult {
                stage: OperationStage::PublishContent,
                status: Outcome::Completed,
                duration_secs: publish_started
                    .lock()
                    .map(|started| started.elapsed().as_secs_f64())
                    .unwrap_or(0.0),
                error: None,
            });
        }
        result
    }

    /// Lists publications that must be recovered before launch or reinstall.
    pub async fn pending_recovery(&self, instance: InstanceId) -> Result<Vec<String>> {
        Ok(storage::pending(&self.db, Some(instance))
            .await?
            .into_iter()
            .map(|pending| pending.id)
            .collect())
    }

    /// Tries recovery again after the user resolves a conflicting local file.
    pub async fn retry_recovery(self: &Arc<Self>, instance: InstanceId) -> Result<()> {
        let activity = self
            .activity
            .reserve_content_change(instance)
            .map_err(|error| ContentError::Invalid(error.to_string()))?;
        let service = Arc::clone(self);

        // A caller dropping its future must not release the lease mid-publication.
        async_std::task::spawn(async move {
            let _activity = activity;
            let failures = recovery::recover(
                &service.db,
                &service.paths,
                &service.instances,
                Some(instance),
            )
            .await?;
            storage::clear_completed_for_instance(&service.db, instance).await?;
            match failures.into_iter().next() {
                Some(failure) => Err(failure.error),
                None => Ok(()),
            }
        })
        .await
    }

    /// Stops recovery of one pending installation and keeps all files in place.
    pub async fn abandon_recovery(&self, instance: InstanceId, operation: &str) -> Result<()> {
        let _activity = self
            .activity
            .reserve_content_change(instance)
            .map_err(|error| ContentError::Invalid(error.to_string()))?;
        storage::abandon(&self.db, instance, operation).await
    }

    pub fn cancel(&self, id: Uuid) -> Result<()> {
        let jobs = self
            .jobs
            .lock()
            .map_err(|_| ContentError::Invalid("Job lock poisoned".into()))?;
        let job = jobs
            .get(&id)
            .ok_or_else(|| ContentError::Invalid("Установка не найдена".into()))?;
        if job.snapshot.phase == ContentPhase::Downloading {
            job.cancel.cancel();
        }
        Ok(())
    }

    pub fn latest_job(&self, instance: InstanceId) -> Option<ContentJobSnapshot> {
        self.jobs
            .lock()
            .ok()?
            .values()
            .filter(|job| job.snapshot.instance_id == instance)
            .max_by_key(|job| job.created)
            .map(|job| job.snapshot.clone())
    }
}
