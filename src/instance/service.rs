use std::{str::FromStr, sync::Arc};

use slug::slugify;

use crate::{
    bus::EventBus,
    data::{
        db::Database,
        layout::LauncherPaths,
        registry::operation::{message::status::Outcome, OperationRegistry},
    },
    instance::{
        self,
        activity::InstanceActivity,
        download::{self, manifest::load_cached},
        error::InstanceError,
        event::InstanceEvent,
        init::install::{InstallContext, InstallRequest},
        launch::{prepare, resolved},
        model::{InstanceId, InstanceMod, InstanceRecord, NewInstance},
        mods::resolve_mods,
        registry::InstanceRegistry,
        running::RunPhase,
        settings::{
            service::SettingsService, EffectiveSettings, GlobalSettings, InstanceSettings,
            SettingsPatch,
        },
        storage::{DbInstanceStore, InstanceStore},
    },
    java::{
        model::{JavaEvent, JavaRuntime},
        policy::JavaPolicy,
        registry::JavaRegistry,
        select,
        service::JavaService,
    },
    modloader::model::{LoaderKind, LoaderSpec},
    version::provider::MetaProvider,
};

use crate::instance::Result;

pub struct InstanceService {
    db: Arc<Database>,
    registry: Arc<InstanceRegistry>,
    settings: Arc<SettingsService>,
    java: Arc<JavaRegistry>,
    java_service: Arc<JavaService>,
    operations: Arc<OperationRegistry>,
    paths: Arc<LauncherPaths>,
    activity: Arc<InstanceActivity>,
    mod_index: Arc<instance::content::index::ModIndex>,
    bus: EventBus,
}

impl InstanceService {
    pub(crate) fn tasks(&self) -> crate::utils::jobs::supervisor::Tasks {
        self.operations.tasks.clone()
    }
    pub(crate) fn new(
        db: Arc<Database>,
        settings: Arc<SettingsService>,
        java_service: Arc<JavaService>,
        operations: Arc<OperationRegistry>,
        paths: Arc<LauncherPaths>,
        bus: EventBus,
    ) -> Self {
        let store: Arc<dyn InstanceStore> = Arc::new(DbInstanceStore::new(Arc::clone(&db)));
        let lease = db.layout_lease.clone();

        Self {
            registry: Arc::new(InstanceRegistry::new(store)),
            db,
            settings,
            java: java_service.get_registry(),
            java_service,
            operations: operations.clone(),
            paths,
            activity: Arc::new(InstanceActivity::with_tasks(
                bus.clone(),
                operations.tasks.clone(),
                lease,
            )),
            mod_index: Arc::new(instance::content::index::ModIndex::with_tasks(
                operations.tasks.clone(),
            )),
            bus,
        }
    }

    pub(super) fn activity(&self) -> Arc<InstanceActivity> {
        Arc::clone(&self.activity)
    }

    pub(super) fn mod_index(&self) -> Arc<instance::content::index::ModIndex> {
        Arc::clone(&self.mod_index)
    }

    /// Removes the instance registration. Game files are kept on disk.
    pub async fn unregister(&self, id: InstanceId) -> Result<()> {
        let _ticket = self.operations.tasks.admit("unregister instance", None)?;
        let _activity = self.activity.reserve_content_change(id)?;
        self.ensure_content_recovered(id).await?;
        self.registry.delete(id).await?;
        self.mod_index.forget(id);
        Ok(())
    }

    pub async fn create(
        &self,
        name: String,
        version: String,
        manifest_url: String,
        meta_provider: String,
    ) -> Result<Arc<InstanceRecord>> {
        self.create_with_loader(
            name,
            version,
            LoaderSpec::vanilla(),
            manifest_url,
            meta_provider,
        )
        .await
    }

    pub async fn create_with_loader(
        &self,
        name: String,
        version: String,
        loader: LoaderSpec,
        manifest_url: String,
        meta_provider: String,
    ) -> Result<Arc<InstanceRecord>> {
        let _ticket = self.operations.tasks.admit("create instance", None)?;
        let dir = slugify(&name);

        if dir.is_empty() {
            return Err(InstanceError::CreationFailed(format!(
                "name produces empty dir: {name}"
            )));
        }

        MetaProvider::from_str(&meta_provider)?;
        loader.validate(&version)?;

        let new = NewInstance {
            name,
            version,
            loader: loader.kind,
            loader_version: loader.version,
            manifest_url,
            meta_provider,
            dir,
        };

        let rec = self.registry.create(new).await?;
        self.bus
            .publish(instance::scan::inspect(&rec, &self.paths).await);

        Ok(rec)
    }

    async fn ensure_content_recovered(&self, id: InstanceId) -> Result<()> {
        let pending = instance::content::has_pending_publication(&self.db, id)
            .await
            .map_err(crate::data::db::DbError::from)?;
        if pending {
            return Err(InstanceError::ContentRecoveryPending(id));
        }

        Ok(())
    }

    pub async fn install(self: &Arc<Self>, id: InstanceId) -> Result<()> {
        let job = self.activity.reserve_install(id)?;

        self.ensure_content_recovered(id).await?;

        let service = Arc::clone(self);
        let (tx, rx) = async_channel::bounded(1);

        self.tasks()
            .spawn("instance install", Some(job.cancel.clone()), async move {
                let result = service.install_inner(id, &job.cancel).await;
                let _ = tx.send(result).await;

                drop(job);
            })
            .map_err(InstanceError::Task)?;

        rx.recv().await.map_err(InstanceError::WorkerResult)?
    }

    pub fn cancel_install(&self, id: InstanceId) {
        self.activity.install_jobs.cancel(id);
    }

    async fn install_inner(
        &self,
        id: InstanceId,
        cancel: &download::transfer::Cancel,
    ) -> Result<()> {
        let rec = self.get_instance(id).await?;

        let loader = LoaderSpec {
            kind: rec.loader,
            version: rec.loader_version.clone(),
        };

        loader.validate(&rec.version)?;

        let meta_provider = MetaProvider::from_str(&rec.meta_provider)?;

        let op = self
            .operations
            .begin_for(
                "instance.install".into(),
                instance::init::install::resolve_install_sequence(&loader),
                crate::operations::OperationSubject::InstanceInstall { instance: id },
            )
            .owned_worker();

        self.java_service.wait_discovery().await;
        let java_use = match self.java_service.retain_runtime(cancel).await {
            Ok(java_use) => java_use,
            Err(error) => {
                op.finish(if cancel.is_cancelled() {
                    Outcome::Cancelled
                } else {
                    Outcome::Failed
                });
                return Err(crate::instance::launch::LaunchError::Retention(error).into());
            }
        };
        let java = match self.java.list_all().await {
            Ok(java) => java,
            Err(e) => {
                op.finish(Outcome::Failed);
                java_use.finish(true);
                return Err(e.into());
            }
        };

        let catalog = crate::modloader::catalog::ModloaderCatalog::new(
            self.paths.meta().join("modloader-catalog"),
        );

        let request = InstallRequest {
            instance_id: rec.id,
            instance_dir_name: rec.dir.clone(),
            game_version: rec.version.clone(),
            manifest_url: rec.manifest_url.clone(),
            meta_provider,
            loader,
        };

        let context = InstallContext {
            paths: &self.paths,
            db: Arc::clone(&self.db),
            catalog: &catalog,
            modloader_java_runtimes: &java,
            java_use: &java_use,
            op: &op,
            cancel,
        };

        let result = instance::init::install::execute(&request, &context).await;
        let result = match result {
            Ok(profile) => {
                if let Err(error) = self.auto_select_runtime(&rec, &profile.java, true).await {
                    tracing::warn!(instance = %rec.id, %error, "Java auto-selection after installation failed");
                }
                op.finish(Outcome::Completed);
                Ok(())
            }
            Err(e) => {
                let outcome = if e.is_cancelled() {
                    Outcome::Cancelled
                } else {
                    Outcome::Failed
                };

                op.finish(outcome);
                Err(e.into())
            }
        };
        java_use.finish(true);
        result
    }

    /// After new runtimes appear: picks a Java for every instance that has
    /// none of its own. Requirements come from the cached manifests; an
    /// instance never installed has none and is skipped. Never notifies about
    /// instances still left without one, the user did not ask about them.
    pub async fn assign_missing(&self) {
        let Ok(_ticket) = self.operations.tasks.admit("assign missing Java", None) else {
            return;
        };
        self.assign_missing_retained().await;
    }

    pub(crate) async fn assign_missing_retained(&self) {
        let list = match self.list().await {
            Ok(list) => list,
            Err(e) => {
                tracing::warn!(error = %e, "cannot list instances for java auto-select");
                return;
            }
        };

        for rec in list {
            match self.has_own_java(rec.id).await {
                Ok(false) => {}
                Ok(true) => continue,
                Err(e) => {
                    tracing::warn!(instance = %rec.id, error = %e, "cannot read instance settings");
                    continue;
                }
            }

            let requirement = match instance::init::storage::load(&self.db, rec.id).await {
                Ok(Some(profile)) => profile.java,
                Ok(None) if rec.loader == LoaderKind::Vanilla => {
                    let Some(manifest) = load_cached(&rec.manifest_url, self.paths.meta()).await
                    else {
                        continue;
                    };
                    let majors = crate::version::provider::java_majors(&manifest);
                    JavaPolicy {
                        allow_newer: majors.iter().any(|&major| major > 8),
                        majors,
                    }
                }
                Ok(None) => continue,
                Err(error) => {
                    tracing::warn!(%error,"cannot load installed Java policy");
                    continue;
                }
            };

            if let Err(e) = self.auto_select_runtime(&rec, &requirement, false).await {
                tracing::warn!(instance = %rec.id, error = %e, "java auto-select failed");
            }
        }
    }

    /// Writes the best registered runtime into the instance's own settings,
    /// unless it has one already. Publishes `AutoSelected`, or with `notify`
    /// `NoSuitableRuntime` when nothing fits.
    async fn auto_select_runtime(
        &self,
        rec: &InstanceRecord,
        policy: &JavaPolicy,
        notify: bool,
    ) -> Result<()> {
        self.java_service.wait_discovery().await;
        let java_use = self
            .java_service
            .retain_runtime(&crate::utils::cancellation::Cancellation::new())
            .await
            .map_err(crate::instance::launch::LaunchError::Retention)?;
        let result = self.auto_select_retained(rec, policy, notify).await;
        java_use.finish(true);
        result
    }

    async fn auto_select_retained(
        &self,
        rec: &InstanceRecord,
        policy: &JavaPolicy,
        notify: bool,
    ) -> Result<()> {
        if self.has_own_java(rec.id).await? {
            return Ok(());
        }

        let runtimes = self.java.list_all().await?;

        match select::pick_with_policy(&runtimes, policy, select::host_arch()) {
            Some((java, fit)) => {
                #[cfg(test)]
                tests::pause_auto_selection(self.paths.root()).await;
                let runtime = java.id;
                if !self
                    .settings
                    .assign_missing_java(rec.id, runtime, &self.bus)
                    .await?
                {
                    return Ok(());
                }
                tracing::info!(
                    instance = %rec.id,
                    runtime = java.id,
                    major = java.major,
                    ?fit,
                    "java auto-selected"
                );

                self.bus.publish(JavaEvent::AutoSelected {
                    instance: rec.id,
                    runtime,
                });
            }
            None if notify && !self.has_own_java(rec.id).await? => {
                self.bus.publish(JavaEvent::NoSuitableRuntime {
                    instance: rec.id,
                    name: rec.name.clone(),
                    required: policy.majors.clone(),
                });
            }
            None => {}
        }

        Ok(())
    }

    pub async fn launch(&self, id: InstanceId) -> Result<()> {
        let reservation = self.activity.reserve_launch(id)?;

        self.ensure_content_recovered(id).await?;
        let rec = self.get_instance(id).await?;
        let settings = self.settings.effective(id).await?;
        let java_use = self
            .java_service
            .retain_runtime(&crate::utils::cancellation::Cancellation::new())
            .await
            .map_err(crate::instance::launch::LaunchError::Retention)?;
        let result = async {
            let java = self.resolve_java(id, &settings).await?;

            let root =
                crate::utils::files::child(self.paths.instances(), std::path::Path::new(&rec.dir))?;
            self.mod_index
                .before_launch(&self.db, &self.activity, id, root)
                .await
                .map_err(|source| InstanceError::ContentInventory { id, source })?;

            // A broad durable pin precedes profile/legacy reads. A lazy JVM JAR open
            // remains protected after launcher exit, not only until spawn returns.
            instance::init::retention::attach(
                self.db.clone(),
                self.paths.libraries().clone(),
                crate::utils::cancellation::Cancellation::new(),
            )
            .await?;
            let runtime_store =
                crate::utils::object_store::ObjectStore::new(self.paths.libraries().clone());
            let _runtime_access = runtime_store
                .access(&crate::utils::cancellation::Cancellation::new())
                .await
                .map_err(crate::instance::launch::LaunchError::Retention)?;
            let runtime_pin = crate::utils::process_pin::ProcessPin::create(self.paths.libraries())
                .map_err(crate::instance::launch::LaunchError::Retention)?;
            let mut command = match instance::init::storage::load(&self.db, id).await? {
                Some(profile) => {
                    resolved::prepare(&rec, &settings, &java, &self.paths, profile).await?
                }
                None if rec.loader == LoaderKind::Vanilla => {
                    prepare::prepare_command(
                        &rec,
                        &settings,
                        &java,
                        &self.paths,
                        Arc::clone(&self.db),
                    )
                    .await?
                }
                None => return Err(InstanceError::NotInstalled(id)),
            };
            command.retain_runtime(runtime_pin);
            command.retain_runtime(
                java_use
                    .child_pin()
                    .map_err(crate::instance::launch::LaunchError::Retention)?,
            );
            let (child, natives) = resolved::spawn(command)?;
            reservation.commit_with_natives(child, natives);

            Ok(())
        }
        .await;
        java_use.finish(true);
        result
    }

    pub fn running(&self) -> Vec<(InstanceId, RunPhase)> {
        self.activity.running.snapshot()
    }

    pub fn run_phase(&self, id: InstanceId) -> Option<RunPhase> {
        self.activity.running.phase(id)
    }

    pub fn kill(&self, id: InstanceId) -> Result<()> {
        self.activity.running.kill(id)
    }

    pub async fn list(&self) -> Result<Vec<Arc<InstanceRecord>>> {
        self.registry.list().await
    }

    /// Renames the instance. Only the display name changes: `dir` is the
    /// identity on disk and stays as it was slugified at creation, so a renamed
    /// instance keeps its original folder.
    pub async fn rename(&self, id: InstanceId, new_name: &str) -> Result<Arc<InstanceRecord>> {
        let _ticket = self.operations.tasks.admit("rename instance", None)?;
        let rec = self.registry.rename(id, new_name).await?;

        self.bus.publish(InstanceEvent::OverviewChanged(id));
        self.bus
            .publish(instance::scan::inspect(&rec, &self.paths).await);

        Ok(rec)
    }

    pub async fn mods(&self, id: InstanceId) -> Result<Vec<InstanceMod>> {
        let rec = self.get_instance(id).await?;
        let root =
            crate::utils::files::child(self.paths.instances(), std::path::Path::new(&rec.dir))?;
        resolve_mods(id, root.join("mods")).await
    }

    pub async fn settings(&self, id: InstanceId) -> Result<InstanceSettings> {
        self.settings.raw(id).await
    }

    pub async fn update_settings(&self, id: InstanceId, patch: &SettingsPatch) -> Result<()> {
        let _ticket = self
            .operations
            .tasks
            .admit("update instance settings", None)?;
        self.settings.patch_and_notify(id, patch, &self.bus).await
    }

    pub async fn global_settings(&self) -> Result<GlobalSettings> {
        self.settings.global().await
    }

    /// Whether the instance points at a runtime that still exists. A dangling
    /// id (the runtime was removed) counts as none: auto-selection replaces it.
    async fn has_own_java(&self, id: InstanceId) -> Result<bool> {
        let Some(java_id) = self.settings.raw(id).await?.java_runtime else {
            return Ok(false);
        };

        Ok(self.java.get(java_id).await?.is_some())
    }

    pub(crate) fn get_registry(&self) -> Arc<InstanceRegistry> {
        Arc::clone(&self.registry)
    }

    async fn get_instance(&self, id: InstanceId) -> Result<Arc<InstanceRecord>> {
        Ok(self.registry.get(id).await?)
    }

    async fn resolve_java(
        &self,
        id: InstanceId,
        settings: &EffectiveSettings,
    ) -> Result<Arc<JavaRuntime>> {
        let java_id = settings
            .java_runtime
            .ok_or(InstanceError::JavaRuntimeNotSelected(id))?;

        self.java
            .get(java_id)
            .await?
            .ok_or(InstanceError::JavaRuntimeMissing(java_id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::channel::oneshot;
    use std::{
        collections::HashMap,
        path::{Path, PathBuf},
        sync::{Mutex, OnceLock},
    };

    type SelectionHook = (oneshot::Sender<()>, oneshot::Receiver<()>);
    static SELECTION: OnceLock<Mutex<HashMap<PathBuf, SelectionHook>>> = OnceLock::new();

    pub(super) async fn pause_auto_selection(root: &Path) {
        let hook = SELECTION
            .get_or_init(Default::default)
            .lock()
            .unwrap()
            .remove(root);
        if let Some((entered, resume)) = hook {
            let _ = entered.send(());
            let _ = resume.await;
        }
    }

    #[async_std::test]
    async fn java_auto_selection_preserves_a_manual_choice_made_before_update() {
        let temp = tempfile::tempdir().unwrap();
        let core = crate::GlobalState::new(crate::Config {
            root_override: Some(temp.path().into()),
            discover_java_on_startup: false,
            ..crate::Config::init()
        })
        .await
        .unwrap();
        let instances = core.instances();
        let record = instances
            .create(
                "Fixture".into(),
                "1.20.1".into(),
                "fixture".into(),
                "prism".into(),
            )
            .await
            .unwrap();
        let java = core.java();
        let runtime = |major| crate::java::model::NewJavaRuntime {
            version: Some(format!("{major}.0.1")),
            major: Some(major),
            arch: None,
            exec_path: Some(temp.path().join(format!("java-{major}"))),
            home_path: Some(temp.path().into()),
            vendor: None,
            source: Default::default(),
        };
        java.init_external(runtime(17)).await.unwrap();
        let manual = java.init_external(runtime(21)).await.unwrap();
        let mut events = core.bus.subscribe();
        let (entered, waiting) = oneshot::channel();
        let (resume, gate) = oneshot::channel();
        SELECTION
            .get_or_init(Default::default)
            .lock()
            .unwrap()
            .insert(temp.path().into(), (entered, gate));
        let policy = JavaPolicy {
            majors: vec![17],
            allow_newer: false,
        };
        let automatic = instances.auto_select_retained(&record, &policy, true);
        let choose = async {
            async_std::future::timeout(std::time::Duration::from_secs(10), waiting)
                .await
                .unwrap()
                .unwrap();
            instances
                .update_settings(
                    record.id,
                    &SettingsPatch {
                        java_runtime: Some(Some(manual.id)),
                        ..Default::default()
                    },
                )
                .await
                .unwrap();
            resume.send(()).unwrap();
        };
        let (result, ()) = futures::join!(automatic, choose);
        result.unwrap();
        assert_eq!(
            instances.settings(record.id).await.unwrap().java_runtime,
            Some(manual.id)
        );
        while let Ok(event) = events.try_recv() {
            assert!(!matches!(
                event,
                crate::bus::event::CoreEvent::Java(JavaEvent::AutoSelected { .. })
            ));
        }
        core.shutdown().await;
    }
}
