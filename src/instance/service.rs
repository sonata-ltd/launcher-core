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
    instance::{
        self,
        error::InstanceError,
        launch::{execute::launch_instance, prepare},
        model::{InstanceId, InstanceRecord, NewInstance},
        registry::InstanceRegistry,
        settings::service::SettingsService,
        storage::{DbInstanceStore, InstanceStore},
    },
    java::{model::JavaRuntime, registry::JavaRegistry},
    version::provider::MetaProvider,
};

use crate::instance::Result;

pub struct InstanceService {
    db: Arc<Database>,
    registry: Arc<InstanceRegistry>,
    settings: Arc<SettingsService>,
    java: Arc<JavaRegistry>,
    operations: Arc<OperationRegistry>,
    paths: Arc<LauncherPaths>,
    bus: EventBus,
}

impl InstanceService {
    pub fn new(
        db: Arc<Database>,
        settings: Arc<SettingsService>,
        java: Arc<JavaRegistry>,
        operations: Arc<OperationRegistry>,
        paths: Arc<LauncherPaths>,
        bus: EventBus,
    ) -> Self {
        let store: Arc<dyn InstanceStore> = Arc::new(DbInstanceStore::new(Arc::clone(&db)));

        Self {
            registry: Arc::new(InstanceRegistry::new(store)),
            db,
            settings,
            java,
            operations,
            paths,
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

        self.resolve_java(id).await?;

        let op = self.operations.begin(
            "instance.install".to_string(),
            vec![
                OperationStage::FetchManifest,
                OperationStage::DownloadLibs,
                OperationStage::DownloadAssets,
            ],
        );

        match instance::init::install(&rec, provider, &self.paths, Arc::clone(&self.db), &op).await
        {
            Ok(()) => {
                op.finish(Outcome::Completed);
                Ok(())
            }
            Err(e) => {
                op.finish(Outcome::Failed);
                Err(e)
            }
        }
    }

    pub async fn launch(&self, id: InstanceId) -> Result<()> {
        let rec = self.get_instance(id).await?;
        let settings = self.settings.effective(id).await?;
        let java = self.resolve_java(id).await?;

        let info =
            prepare::prepare(&rec, &settings, &java, &self.paths, Arc::clone(&self.db)).await?;

        launch_instance(info).await?;

        Ok(())
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

    pub(crate) fn get_registry(&self) -> Arc<InstanceRegistry> {
        Arc::clone(&self.registry)
    }

    async fn get_instance(&self, id: InstanceId) -> Result<Arc<InstanceRecord>> {
        Ok(self.registry.get(id).await?)
    }

    async fn resolve_java(&self, id: InstanceId) -> Result<Arc<JavaRuntime>> {
        let settings = self.settings.effective(id).await?;

        let java_id = settings
            .java_runtime
            .ok_or(InstanceError::JavaRuntimeNotSelected(id))?;

        self.java
            .get(java_id)
            .await?
            .ok_or(InstanceError::JavaRuntimeMissing(java_id))
    }
}
