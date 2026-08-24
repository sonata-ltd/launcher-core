use std::sync::Arc;

use crate::{
    data::db::{Database, DbError},
    instance::{
        error::InstanceError,
        model::InstanceId,
        settings::{
            storage::{DbInstanceSettingsStore, InstanceSettingsStore},
            EffectiveSettings, GlobalSettings, InstanceSettings, SettingsPatch,
        },
        Result,
    },
};

pub struct SettingsService {
    storage: Arc<dyn InstanceSettingsStore>,
}

impl SettingsService {
    pub fn new(db: Arc<Database>) -> Self {
        Self {
            storage: Arc::new(DbInstanceSettingsStore::new(db)),
        }
    }

    pub async fn raw(&self, id: InstanceId) -> Result<InstanceSettings> {
        self.storage
            .get(id)
            .await?
            .ok_or(InstanceError::NotFound(id))
    }

    pub async fn global(&self) -> Result<GlobalSettings> {
        Ok(self.storage.global().await?)
    }

    pub async fn effective(&self, id: InstanceId) -> Result<EffectiveSettings> {
        let instance = self.raw(id).await?;
        let global = self.global().await?;

        Ok(EffectiveSettings::resolve(&global, &instance))
    }

    pub async fn patch(&self, id: InstanceId, patch: &SettingsPatch) -> Result<()> {
        let updated = self.storage.patch(id, patch).await.map_err(|e| match e {
            DbError::InvalidInput(_) => match patch.java_runtime.flatten() {
                Some(java_id) => InstanceError::JavaRuntimeMissing(java_id),
                None => InstanceError::DB(e),
            },
            other => InstanceError::DB(other),
        })?;

        if !updated {
            return Err(InstanceError::NotFound(id));
        }

        Ok(())
    }
}
