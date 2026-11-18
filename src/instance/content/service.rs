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
use crate::utils::jobs::history::History;
use crate::{
    data::{db::Database, layout::LauncherPaths},
    instance::content::{
        catalog::{types::*, ContentCatalog},
        error::{BusyReason, ContentError, ContentInitError, Result, StalePlanReason},
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
use futures::FutureExt;
use uuid::Uuid;

const ACTIVE_LIMIT: usize = 128;
const HISTORY_LIMIT: usize = 128;
const HISTORY_TTL: Duration = Duration::from_secs(3600);
const PLAN_TTL: Duration = Duration::from_secs(300);

fn terminal_phase(error: Option<&ContentError>, previous: ContentPhase) -> ContentPhase {
    match error {
        None => ContentPhase::Completed,
        Some(error) if error.is_cancelled() && previous == ContentPhase::Downloading => {
            ContentPhase::Cancelled
        }
        Some(_) => ContentPhase::Failed,
    }
}

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
    completed: Option<Instant>,
}

impl Job {
    #[cfg(test)]
    fn retained_at(&self, now: Instant) -> bool {
        self.snapshot.active()
            || self.completed.is_none_or(|completed| {
                now.saturating_duration_since(completed) < Duration::from_secs(3600)
            })
    }

    fn finish(&mut self, phase: ContentPhase, error: Option<Arc<ContentError>>) {
        self.snapshot.phase = phase;
        self.snapshot.error = error.as_ref().map(ToString::to_string);
        self.snapshot.cause = error;
        self.completed = Some(Instant::now());
        self.snapshot.finished_at = Some(chrono::Utc::now());
    }
}

struct Jobs {
    active: HashMap<Uuid, Job>,
    history: History<Uuid, Job>,
}

struct TerminalGuard {
    jobs: Arc<Mutex<Jobs>>,
    id: Uuid,
}
impl Drop for TerminalGuard {
    fn drop(&mut self) {
        self.jobs
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .finish(
                self.id,
                ContentPhase::Failed,
                Some(Arc::new(ContentError::WorkerPanicked)),
            );
    }
}

impl Jobs {
    fn new() -> Self {
        Self {
            active: HashMap::new(),
            history: History::new(HISTORY_LIMIT, HISTORY_TTL),
        }
    }

    fn admit(&mut self, id: Uuid, job: Job) -> Result<()> {
        if self.active.len() >= ACTIVE_LIMIT {
            return Err(BusyReason::ActiveLimit(ACTIVE_LIMIT).into());
        }
        self.active.insert(id, job);
        Ok(())
    }

    fn finish(&mut self, id: Uuid, phase: ContentPhase, error: Option<Arc<ContentError>>) {
        if let Some(mut job) = self.active.remove(&id) {
            job.finish(phase, error);
            self.history.push(id, job);
        }
    }

    fn latest(&mut self, instance: InstanceId) -> Option<ContentJobSnapshot> {
        self.history.prune();
        self.active
            .values()
            .chain(self.history.records().map(|record| &record.value))
            .filter(|job| job.snapshot.instance_id == instance)
            .max_by_key(|job| job.created)
            .map(|job| job.snapshot.clone())
    }
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
    plans: Arc<Mutex<HashMap<Uuid, StoredPlan>>>,
    jobs: Arc<Mutex<Jobs>>,
    tasks: crate::utils::jobs::supervisor::Tasks,
    // OS-lock защищает от второго обновлённого процесса с тем же корнем.
    // In-memory барьер InstanceService защищает launch и install внутри процесса.
    _root_lock: Arc<crate::utils::object_store::StoreLock>,
    _instances_lock: Arc<crate::utils::object_store::StoreLock>,
    _store_lock: Arc<crate::utils::object_store::StoreLock>,
}

impl InstanceContentService {
    pub(crate) async fn new(
        db: Arc<Database>,
        paths: Arc<LauncherPaths>,
        instances: Arc<InstanceService>,
        curseforge_key: Option<String>,
        operations: Arc<crate::data::registry::operation::OperationRegistry>,
    ) -> std::result::Result<Self, ContentInitError> {
        let root_lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(paths.root().join(".launcher-content.lock"))?;
        let root_lock =
            crate::utils::object_store::StoreLock::exclusive(root_lock).map_err(|source| {
                ContentInitError::Lock {
                    path: paths.root().clone(),
                    source,
                }
            })?;
        let root_lock = Arc::new(root_lock);
        // The configured launcher root is not the instances resource identity:
        // different roots may override instances to the same directory/alias.
        std::fs::create_dir_all(paths.instances())?;
        let lock_path = files::child(
            paths.instances(),
            std::path::Path::new(".launcher-content.lock"),
        )?;
        let instances_lock = crate::utils::object_store::StoreLock::exclusive(
            OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .open(lock_path)?,
        )
        .map_err(|source| ContentInitError::Lock {
            path: paths.instances().clone(),
            source,
        })?;
        let instances_lock = Arc::new(instances_lock);
        let registry = instances.get_registry();
        let activity = instances.activity();
        let tasks = instances.tasks();
        let store = Arc::new(ObjectStore::new(paths.root().join("content-store")));
        let store_lock = Arc::new(store.lock_exclusive()?);
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
        if let Err(error) = super::store::legacy::detach_registered_links(
            db.clone(),
            paths.clone(),
            activity.clone(),
            store.clone(),
            [
                root_lock.clone(),
                instances_lock.clone(),
                store_lock.clone(),
            ],
            &tasks,
        )
        .await
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
        let plans = Arc::new(Mutex::new(HashMap::<Uuid, StoredPlan>::new()));
        let jobs = Arc::new(Mutex::new(Jobs::new()));
        let weak_plans = Arc::downgrade(&plans);
        let weak_jobs = Arc::downgrade(&jobs);
        let stop = Cancel::default();
        let stopped = stop.clone();
        tasks
            .spawn("content history pruning", Some(stop), async move {
                loop {
                    if stopped
                        .run(async_std::task::sleep(Duration::from_secs(60)))
                        .await
                        .is_err()
                    {
                        break;
                    }
                    let (Some(plans), Some(jobs)) = (weak_plans.upgrade(), weak_jobs.upgrade())
                    else {
                        break;
                    };
                    plans
                        .lock()
                        .unwrap_or_else(|error| error.into_inner())
                        .retain(|_, plan| plan.created.elapsed() < PLAN_TTL);
                    jobs.lock()
                        .unwrap_or_else(|error| error.into_inner())
                        .history
                        .prune();
                }
            })
            .map_err(ContentInitError::Task)?;
        Ok(Self {
            operations,
            catalog: ContentCatalog::new(curseforge_key),
            instances: registry,
            mod_index: instances.mod_index(),
            activity,
            paths: paths.clone(),
            db,
            store,
            plans,
            jobs,
            tasks,
            _root_lock: root_lock,
            _instances_lock: instances_lock,
            _store_lock: store_lock,
        })
    }

    async fn context(&self, instance: InstanceId) -> Result<(PathBuf, Compatibility)> {
        let record = self
            .instances
            .get(instance)
            .await
            .map_err(ContentError::from)?;
        if matches!(
            record.loader,
            crate::modloader::model::LoaderKind::Vanilla
                | crate::modloader::model::LoaderKind::Cleanroom
        ) {
            return Err(ContentError::Invalid(
                "automatic mod installation is unsupported for this loader".into(),
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
                        "multiple active releases of the same project".into(),
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
        .map_err(|_| ContentError::Timeout(Duration::from_secs(90)))??;
        let mut names = std::collections::HashSet::new();
        let mut releases = Vec::new();
        let mut items = already_present;
        for release in all_releases {
            files::portable_name::validate_file_name(&release.file.file_name)?;
            // Переносимое консервативное ограничение для создаваемых имён.
            if !names.insert(release.file.file_name.to_lowercase()) {
                return Err(ContentError::Invalid(
                    "different projects create the same file name".into(),
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
                        "{} already exists with different bytes",
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
                    "another release of the project is already installed".into(),
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
            return Err(ContentError::StalePlan(StalePlanReason::InventoryChanged));
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
        plans.retain(|_, plan| plan.created.elapsed() < PLAN_TTL);
        if plans.len() >= 32 {
            return Err(BusyReason::PreviewLimit(32).into());
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
        let plan = {
            let mut plans = self
                .plans
                .lock()
                .map_err(|_| ContentError::Invalid("Plan lock poisoned".into()))?;
            plans.retain(|_, plan| plan.created.elapsed() < PLAN_TTL);
            plans.remove(&plan_id)
        }
        .ok_or(ContentError::StalePlan(StalePlanReason::Expired))?;
        if plan.created.elapsed() >= Duration::from_secs(300) {
            return Err(ContentError::StalePlan(StalePlanReason::Expired));
        }
        if plan.releases.is_empty() {
            return Err(ContentError::Invalid(
                "all selected files are already installed".into(),
            ));
        }
        let instance = plan.preview.instance_id;
        let activity = self
            .activity
            .reserve_content_change(instance)
            .map_err(ContentError::from)?;
        let id = Uuid::new_v4();
        use crate::data::registry::operation::message::stage::OperationStage;
        let operation = self
            .operations
            .begin_for(
                format!("instance.content.install:{}", instance.get()),
                vec![
                    OperationStage::DownloadContent,
                    OperationStage::PublishContent,
                ],
                crate::operations::OperationSubject::ContentInstall {
                    instance,
                    installation: id,
                },
            )
            .owned_worker();
        let operation_id = operation.id();
        let cancel = Cancel::default();
        {
            let mut jobs = self
                .jobs
                .lock()
                .map_err(|_| ContentError::Invalid("Job lock poisoned".into()))?;
            jobs.admit(
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
                        cause: None,
                        finished_at: None,
                    },
                    cancel: cancel.clone(),
                    created: Instant::now(),
                    completed: None,
                },
            )?;
        }
        let service = self.clone();
        self.tasks.spawn("content installation", Some(cancel.clone()), async move {
            // Job владеет activity, а не GUI Task: закрытие страницы не даёт
            // запустить игру посередине установки и не теряет результат.
            let _activity = activity;
            let _terminal = TerminalGuard { jobs: service.jobs.clone(), id };
            use crate::data::registry::operation::message::status::Outcome;
            operation.start_stage_determinable(OperationStage::DownloadContent);
            let result = std::panic::AssertUnwindSafe(service.execute_plan(id, plan, &cancel, &operation)).catch_unwind().await
                .unwrap_or_else(|_| Err(ContentError::WorkerPanicked));
            let mut error = result.err().map(Arc::new);
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
                            error = Some(Arc::new(failure.error));
                        } else if storage::operation_completed(&service.db, id)
                            .await
                            .unwrap_or(false)
                        {
                            error = None;
                        }
                    }
                    Err(recovery_error) => error = Some(Arc::new(recovery_error)),
                }
            }
            let mut phase = terminal_phase(error.as_deref(), ContentPhase::Publishing);
            if let Ok(mut jobs) = service.jobs.lock() {
                if let Some(job) = jobs.active.get(&id) {
                    phase = terminal_phase(error.as_deref(), job.snapshot.phase);
                    jobs.finish(id, phase, error.clone());
                }
            }
            let outcome = if error.is_none() {
                Outcome::Completed
            } else if phase == ContentPhase::Cancelled {
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
        }).map_err(|error| {
            self.jobs.lock().unwrap_or_else(|error| error.into_inner()).finish(id, ContentPhase::Failed, Some(Arc::new(ContentError::Task(error.clone()))));
            ContentError::Task(error)
        })?;
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
            return Err(ContentError::StalePlan(
                StalePlanReason::CompatibilityChanged,
            ));
        }
        storage::ensure_no_pending(&self.db, plan.preview.instance_id).await?;
        let snapshot = super::inventory::snapshot(root.clone()).await?;
        if snapshot.signature != plan.signature {
            return Err(ContentError::StalePlan(StalePlanReason::InventoryChanged));
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
                    if let Some(job) = jobs.active.get_mut(&id) {
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
            .map_err(ContentError::from)?;
        let service = Arc::clone(self);

        // A caller dropping its future must not release the lease mid-publication.
        self.tasks
            .spawn("content recovery", None, async move {
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
            .map_err(ContentError::Task)?
            .await
            .map_err(ContentError::Task)?
    }

    /// Stops recovery of one pending installation and keeps all files in place.
    pub async fn abandon_recovery(&self, instance: InstanceId, operation: &str) -> Result<()> {
        let activity = self
            .activity
            .reserve_content_change(instance)
            .map_err(ContentError::from)?;
        let db = self.db.clone();
        let operation = operation.to_owned();
        self.tasks
            .spawn("content abandon recovery", None, async move {
                let _activity = activity;
                storage::abandon(&db, instance, &operation).await
            })
            .map_err(ContentError::Task)?
            .await
            .map_err(ContentError::Task)?
    }

    pub fn cancel(&self, id: Uuid) -> Result<()> {
        let jobs = self
            .jobs
            .lock()
            .map_err(|_| ContentError::Invalid("Job lock poisoned".into()))?;
        if jobs.history.records().any(|record| record.id == id) {
            return Ok(());
        }
        let job = jobs.active.get(&id).ok_or(ContentError::JobNotFound(id))?;
        if job.snapshot.phase == ContentPhase::Downloading {
            job.cancel.cancel();
        }
        Ok(())
    }

    pub fn latest_job(&self, instance: InstanceId) -> Option<ContentJobSnapshot> {
        self.jobs.lock().ok()?.latest(instance)
    }
}

#[cfg(test)]
mod history_tests {
    use super::*;

    #[test]
    fn terminal_outcome_uses_failure_cause_not_a_cancelled_token_and_history_keeps_cause() {
        let cancel = Cancel::default();
        cancel.cancel();
        let cause = Arc::new(ContentError::Io(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "cancelled",
        )));
        assert_eq!(
            terminal_phase(Some(&cause), ContentPhase::Downloading),
            ContentPhase::Failed
        );
        let cancelled = ContentError::from(crate::utils::cancellation::Cancelled);
        assert_eq!(
            terminal_phase(Some(&cancelled), ContentPhase::Downloading),
            ContentPhase::Cancelled
        );
        assert_eq!(
            terminal_phase(Some(&cancelled), ContentPhase::Publishing),
            ContentPhase::Failed
        );
        assert_eq!(
            terminal_phase(None, ContentPhase::Publishing),
            ContentPhase::Completed
        );
        let mut jobs = Jobs::new();
        let job = job(Instant::now());
        let id = job.snapshot.id;
        jobs.admit(id, job).unwrap();
        jobs.finish(id, ContentPhase::Failed, Some(cause.clone()));
        let snapshot = jobs.latest(1.into()).unwrap();
        assert!(Arc::ptr_eq(snapshot.cause.as_ref().unwrap(), &cause));
        assert_eq!(snapshot.error, Some(cause.to_string()));
    }

    fn job(created: Instant) -> Job {
        let operations =
            crate::data::registry::operation::OperationRegistry::new(crate::bus::EventBus::new(1));
        let operation = operations.begin("fixture".into(), vec![]);
        Job {
            snapshot: ContentJobSnapshot {
                operation_id: operation.id(),
                id: Uuid::new_v4(),
                instance_id: 1.into(),
                phase: ContentPhase::Downloading,
                completed_files: 0,
                total_files: 1,
                error: None,
                cause: None,
                finished_at: None,
            },
            created,
            cancel: Cancel::default(),
            completed: None,
        }
    }

    #[test]
    fn long_installation_has_full_completion_ttl_for_every_terminal_phase() {
        for phase in [
            ContentPhase::Completed,
            ContentPhase::Failed,
            ContentPhase::Cancelled,
        ] {
            let now = Instant::now();
            let mut job = job(now - Duration::from_secs(7200));
            assert!(job.retained_at(now));
            assert!(job.snapshot.finished_at.is_none());
            job.finish(
                phase,
                Some(Arc::new(ContentError::Invalid("fixture".into()))),
            );
            let completed = job.completed.unwrap();
            assert!(job.snapshot.finished_at.is_some());
            assert!(job.retained_at(completed + Duration::from_secs(3599)));
            assert!(!job.retained_at(completed + Duration::from_secs(3600)));
            assert!(job.created < completed);
        }
    }

    #[test]
    fn full_completed_ring_never_rejects_new_active_jobs_and_active_limit_is_independent() {
        let mut jobs = Jobs::new();
        for _ in 0..1000 {
            let job = job(Instant::now());
            let id = job.snapshot.id;
            jobs.admit(id, job).unwrap();
            jobs.finish(id, ContentPhase::Completed, None);
        }
        assert_eq!(jobs.history.records().count(), HISTORY_LIMIT);
        for _ in 0..ACTIVE_LIMIT {
            let job = job(Instant::now());
            jobs.admit(job.snapshot.id, job).unwrap();
        }
        let extra = job(Instant::now());
        assert!(jobs.admit(extra.snapshot.id, extra).is_err());
        let id = *jobs.active.keys().next().unwrap();
        jobs.finish(id, ContentPhase::Cancelled, None);
        let extra = job(Instant::now());
        jobs.admit(extra.snapshot.id, extra).unwrap();
        assert_eq!(jobs.history.records().count(), HISTORY_LIMIT);
    }

    #[test]
    fn latest_preserves_creation_order_when_an_older_job_finishes_last() {
        let mut jobs = Jobs::new();
        let old = job(Instant::now() - Duration::from_secs(7200));
        let old_id = old.snapshot.id;
        let new = job(Instant::now());
        let new_id = new.snapshot.id;
        jobs.admit(old_id, old).unwrap();
        jobs.admit(new_id, new).unwrap();
        jobs.finish(new_id, ContentPhase::Completed, None);
        jobs.finish(old_id, ContentPhase::Failed, None);
        assert_eq!(jobs.latest(1.into()).unwrap().id, new_id);
        assert_eq!(jobs.active.len(), 0);
    }
}
