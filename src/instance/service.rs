use std::{str::FromStr, sync::Arc};

use slug::slugify;

use crate::{
    bus::EventBus,
    data::{
        db::Database,
        layout::LauncherPaths,
        registry::operation::{
            message::{stage::OperationStage, status::Outcome},
            OperationRegistry,
        },
    },
    instance::download::manifest::load_cached,
    instance::{
        self,
        error::InstanceError,
        event::InstanceEvent,
        launch::{execute::spawn_instance, prepare},
        model::{InstanceId, InstanceRecord, NewInstance},
        registry::InstanceRegistry,
        running::{RunPhase, RunningInstances},
        settings::{
            service::SettingsService, EffectiveSettings, GlobalSettings, InstanceSettings,
            SettingsPatch,
        },
        storage::{DbInstanceStore, InstanceStore},
    },
    java::{
        model::{JavaEvent, JavaRuntime},
        registry::JavaRegistry,
        select::{self, JavaRequirement},
        service::JavaService,
    },
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
    running: RunningInstances,
    bus: EventBus,
}

impl InstanceService {
    pub fn new(
        db: Arc<Database>,
        settings: Arc<SettingsService>,
        java_service: Arc<JavaService>,
        operations: Arc<OperationRegistry>,
        paths: Arc<LauncherPaths>,
        bus: EventBus,
    ) -> Self {
        let store: Arc<dyn InstanceStore> = Arc::new(DbInstanceStore::new(Arc::clone(&db)));

        Self {
            registry: Arc::new(InstanceRegistry::new(store)),
            db,
            settings,
            java: java_service.get_registry(),
            java_service,
            operations,
            paths,
            running: RunningInstances::new(bus.clone()),
            bus,
        }
    }

    pub async fn create(
        &self,
        name: String,
        version: String,
        loader: String,
        manifest_url: String,
        meta_provider: String,
    ) -> Result<Arc<InstanceRecord>> {
        let dir = slugify(&name);

        if dir.is_empty() {
            return Err(InstanceError::CreationFailed(format!(
                "name produces empty dir: {name}"
            )));
        }

        MetaProvider::from_str(&meta_provider)?;

        let new = NewInstance {
            name,
            version,
            loader,
            manifest_url,
            meta_provider,
            dir,
        };

        let rec = self.registry.create(new).await?;

        self.bus.publish(rec.to_scan_msg());

        Ok(rec)
    }

    pub async fn install(&self, id: InstanceId) -> Result<()> {
        let rec = self.get_instance(id).await?;
        let provider = MetaProvider::from_str(&rec.meta_provider)?;

        let op = self.operations.begin(
            "instance.install".to_string(),
            vec![
                OperationStage::FetchManifest,
                OperationStage::DownloadLibs,
                OperationStage::DownloadAssets,
            ],
        );

        let requirement =
            match instance::init::install(&rec, provider, &self.paths, Arc::clone(&self.db), &op)
                .await
            {
                Ok(requirement) => {
                    op.finish(Outcome::Completed);
                    requirement
                }
                Err(e) => {
                    op.finish(Outcome::Failed);
                    return Err(e);
                }
            };

        // Outside the operation: an instance without a fitting Java is
        // installed all the same, the user is told by `NoSuitableRuntime`.
        self.auto_select(&rec, &requirement, true).await?;

        Ok(())
    }

    /// After new runtimes appear: picks a Java for every instance that has
    /// none of its own. Requirements come from the cached manifests; an
    /// instance never installed has none and is skipped. Never notifies about
    /// instances still left without one, the user did not ask about them.
    pub async fn assign_missing(&self) {
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

            let Some(manifest) = load_cached(&rec.manifest_url, self.paths.meta()).await else {
                tracing::debug!(instance = %rec.id, "no cached manifest, not installed yet");
                continue;
            };

            let requirement = JavaRequirement::from_manifest(&manifest);

            if let Err(e) = self.auto_select(&rec, &requirement, false).await {
                tracing::warn!(instance = %rec.id, error = %e, "java auto-select failed");
            }
        }
    }

    /// Writes the best registered runtime into the instance's own settings,
    /// unless it has one already. Publishes `AutoSelected`, or with `notify`
    /// `NoSuitableRuntime` when nothing fits.
    async fn auto_select(
        &self,
        rec: &InstanceRecord,
        requirement: &JavaRequirement,
        notify: bool,
    ) -> Result<()> {
        self.java_service.wait_discovery().await;

        if self.has_own_java(rec.id).await? {
            return Ok(());
        }

        let runtimes = self.java.list_all().await?;

        match select::pick(&runtimes, requirement, select::host_arch()) {
            Some((java, fit)) => {
                tracing::info!(
                    instance = %rec.id,
                    runtime = java.id,
                    major = java.major,
                    ?fit,
                    "java auto-selected"
                );

                let runtime = java.id;
                let patch = SettingsPatch {
                    java_runtime: Some(Some(runtime)),
                    ..Default::default()
                };

                self.update_settings(rec.id, &patch).await?;
                self.bus.publish(JavaEvent::AutoSelected {
                    instance: rec.id,
                    runtime,
                });
            }
            None if notify => {
                self.bus.publish(JavaEvent::NoSuitableRuntime {
                    instance: rec.id,
                    name: rec.name.clone(),
                    required: requirement.majors.clone(),
                });
            }
            None => {}
        }

        Ok(())
    }

    pub async fn launch(&self, id: InstanceId) -> Result<()> {
        let reservation = self.running.reserve(id)?;

        let rec = self.get_instance(id).await?;
        let settings = self.settings.effective(id).await?;
        let java = self.resolve_java(id, &settings).await?;

        let info =
            prepare::prepare(&rec, &settings, &java, &self.paths, Arc::clone(&self.db)).await?;

        let child = spawn_instance(info).await?;
        reservation.commit(child);

        Ok(())
    }

    pub fn running(&self) -> Vec<(InstanceId, RunPhase)> {
        self.running.snapshot()
    }

    pub fn run_phase(&self, id: InstanceId) -> Option<RunPhase> {
        self.running.phase(id)
    }

    pub fn kill(&self, id: InstanceId) -> Result<()> {
        self.running.kill(id)
    }

    pub async fn list(&self) -> Result<Vec<Arc<InstanceRecord>>> {
        self.registry.list().await
    }

    /// Renames the instance. Only the display name changes: `dir` is the
    /// identity on disk and stays as it was slugified at creation, so a renamed
    /// instance keeps its original folder.
    pub async fn rename(&self, id: InstanceId, new_name: &str) -> Result<Arc<InstanceRecord>> {
        let rec = self.registry.rename(id, new_name).await?;

        self.bus.publish(rec.to_scan_msg());

        Ok(rec)
    }

    pub async fn settings(&self, id: InstanceId) -> Result<InstanceSettings> {
        self.settings.raw(id).await
    }

    pub async fn update_settings(&self, id: InstanceId, patch: &SettingsPatch) -> Result<()> {
        let res = self.settings.patch(id, patch).await?;

        self.bus.publish(InstanceEvent::SettingsChanged(id));

        Ok(res)
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
