use std::sync::Arc;

use crate::{
    bus::EventBus,
    data::db::Database,
    instance::{
        error::InstanceError,
        model::InstanceId,
        options::{
            error::{OptionsError, Result},
            model::{
                OptionUpdateMessage, Overview, OverviewPatch, PageView, Settings, SettingsPatch,
            },
            storage::{DbOptionsStore, OptionsStore},
        },
        registry::InstanceRegistry,
        settings::{service::SettingsService, EffectiveSettings},
    },
    java::registry::JavaRegistry,
};

pub struct OptionsService {
    storage: Arc<dyn OptionsStore>,
    instances: Arc<InstanceRegistry>,
    settings: Arc<SettingsService>,
    java: Arc<JavaRegistry>,
    bus: EventBus,
}

impl OptionsService {
    pub fn new(
        db: Arc<Database>,
        instances: Arc<InstanceRegistry>,
        settings: Arc<SettingsService>,
        java: Arc<JavaRegistry>,
        bus: EventBus,
    ) -> Self {
        Self {
            storage: Arc::new(DbOptionsStore::new(db)),
            instances,
            settings,
            java,
            bus,
        }
    }

    pub async fn overview(&self, id: InstanceId) -> Result<Overview> {
        self.storage
            .overview(id)
            .await?
            .ok_or(OptionsError::InstanceNotFound(id))
    }

    pub async fn settings(&self, id: InstanceId) -> Result<Settings> {
        let rec = self.instances.get(id).await.map_err(map_instance_error)?;

        let overrides = self.settings.raw(id).await.map_err(map_instance_error)?;
        let global = self.settings.global().await.map_err(map_instance_error)?;
        let effective = EffectiveSettings::resolve(&global, &overrides);

        let java_runtime = match effective.java_runtime {
            Some(java_id) => self.java.get(java_id).await?.map(|j| (*j).clone()),
            None => None,
        };

        Ok(Settings {
            dir: rec.dir.clone(),
            overrides,
            effective,
            java_runtime,
        })
    }

    pub async fn patch_overview(&self, id: InstanceId, patch: OverviewPatch) -> Result<Overview> {
        if patch.is_empty() {
            return self.overview(id).await;
        }

        if patch.has_fields() && !self.storage.patch_overview(id, &patch).await? {
            return Err(OptionsError::InstanceNotFound(id));
        }

        if let Some(name) = patch.name.as_deref() {
            self.instances
                .rename(id, name)
                .await
                .map_err(map_instance_error)?;
        }

        let view = self
            .storage
            .overview(id)
            .await?
            .ok_or(OptionsError::InstanceNotFound(id))?;
        self.publish(id, PageView::Overview(view.clone()));

        Ok(view)
    }

    pub async fn patch_settings(&self, id: InstanceId, patch: SettingsPatch) -> Result<Settings> {
        self.settings
            .patch(id, &patch)
            .await
            .map_err(map_instance_error)?;

        let view = self.settings(id).await?;
        self.publish(id, PageView::Settings(view.clone()));

        Ok(view)
    }

    fn publish(&self, instance_id: InstanceId, view: PageView) {
        self.bus.publish(OptionUpdateMessage { instance_id, view });
    }
}

fn map_instance_error(e: InstanceError) -> OptionsError {
    match e {
        InstanceError::NameTaken => OptionsError::NameTaken,
        InstanceError::NotFound(id) => OptionsError::InstanceNotFound(id),
        InstanceError::JavaRuntimeMissing(java_id) => OptionsError::JavaRuntimeNotFound(java_id),
        InstanceError::DB(db) => OptionsError::Db(db),
        InstanceError::Java(java) => OptionsError::Java(java),
        other => OptionsError::Internal(other.to_string()),
    }
}
