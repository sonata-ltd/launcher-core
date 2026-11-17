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
    updates: async_std::sync::Mutex<()>,
}

impl SettingsService {
    pub fn new(db: Arc<Database>) -> Self {
        Self {
            storage: Arc::new(DbInstanceSettingsStore::new(db)),
            updates: Default::default(),
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

        EffectiveSettings::try_resolve(&global, &instance)
    }

    /// Applies a patch and emits the same domain event for every service caller.
    pub(in crate::instance) async fn patch_and_notify(
        &self,
        id: InstanceId,
        patch: &SettingsPatch,
        bus: &crate::bus::EventBus,
    ) -> Result<()> {
        self.patch(id, patch).await?;
        bus.publish(crate::instance::event::InstanceEvent::SettingsChanged(id));
        Ok(())
    }

    pub async fn patch(&self, id: InstanceId, patch: &SettingsPatch) -> Result<()> {
        let _updates = self.updates.lock().await;
        if patch.memory_min.is_some() || patch.memory_max.is_some() || patch.jvm_args.is_some() {
            let mut instance = self.raw(id).await?;
            let global = self.global().await?;
            if patch.memory_min.is_some() || patch.memory_max.is_some() {
                instance.memory_min = patch.memory_min.unwrap_or(instance.memory_min);
                instance.memory_max = patch.memory_max.unwrap_or(instance.memory_max);
                EffectiveSettings::validate_memory(&global, &instance)?;
            }
            if let Some(args) = &patch.jvm_args {
                instance.jvm_args.clone_from(args);
                EffectiveSettings::parse_jvm_args(&global, &instance)?;
            }
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[async_std::test]
    async fn patch_validates_inherited_memory_and_launch_preserves_quoted_arguments() {
        let db = Arc::new(Database::init("sqlite::memory:").await.unwrap());
        sqlx::query("INSERT INTO instances(id,name,dir,version,loader,manifest_url,meta_provider) VALUES(1,'Test','test','1.21.1','vanilla','','prism')")
            .execute(&db.pool).await.unwrap();
        sqlx::query("INSERT INTO instances_settings(instance_id) VALUES(1)")
            .execute(&db.pool)
            .await
            .unwrap();
        let service = SettingsService::new(db);
        let id = InstanceId::from(1);
        assert!(matches!(
            service
                .patch(
                    id,
                    &SettingsPatch {
                        memory_min: Some(Some(5000)),
                        ..Default::default()
                    }
                )
                .await,
            Err(InstanceError::InvalidMemoryRange { .. })
        ));
        assert!(service.raw(id).await.unwrap().memory_min.is_none());
        assert!(matches!(
            service
                .patch(
                    id,
                    &SettingsPatch {
                        jvm_args: Some(Some("-Dlabel=\"unfinished".into())),
                        ..Default::default()
                    }
                )
                .await,
            Err(InstanceError::InvalidJvmArguments)
        ));
        service
            .patch(
                id,
                &SettingsPatch {
                    jvm_args: Some(Some("-Dlabel=\"two words\" -ea".into())),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(
            service.effective(id).await.unwrap().jvm_args,
            ["-Dlabel=two words", "-ea"]
        );
    }
}
