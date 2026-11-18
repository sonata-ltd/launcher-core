use std::sync::Arc;

use crate::{
    bus::EventBus,
    data::db::{Database, DbError},
    instance::{
        error::InstanceError,
        event::InstanceEvent,
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

        let updates = self.instances.lock_updates().await;
        if patch.name.is_some() {
            self.instances.invalidate(id);
        }
        let applied =
            self.storage
                .patch_overview(id, &patch)
                .await
                .map_err(|error| match error {
                    DbError::Conflict(_) => OptionsError::NameTaken,
                    other => OptionsError::Db(other),
                })?;
        if !applied {
            return Err(OptionsError::InstanceNotFound(id));
        }

        let view = self
            .storage
            .overview(id)
            .await?
            .ok_or(OptionsError::InstanceNotFound(id))?;
        drop(updates);
        self.bus.publish(InstanceEvent::OverviewChanged(id));
        self.publish(id, PageView::Overview(view.clone()));

        Ok(view)
    }

    pub async fn patch_settings(&self, id: InstanceId, patch: SettingsPatch) -> Result<Settings> {
        self.settings
            .patch_and_notify(id, &patch, &self.bus)
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
        error @ (InstanceError::InvalidMemoryRange { .. } | InstanceError::InvalidJvmArguments) => {
            OptionsError::InvalidSettings(error)
        }
        other => OptionsError::Internal(Box::new(other)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        instance::{model::NewInstance, storage::DbInstanceStore},
        java::storage::DbJavaStore,
        modloader::model::LoaderKind,
    };

    #[test]
    fn options_mapping_keeps_unexpected_instance_cause_without_text_classification() {
        let error = map_instance_error(InstanceError::Io(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "cancelled UNIQUE constraint failed",
        )));
        let OptionsError::Internal(source) = error else {
            panic!("expected typed instance source");
        };
        let InstanceError::Io(source) = *source else {
            panic!("lost IO source");
        };
        assert_eq!(source.kind(), std::io::ErrorKind::PermissionDenied);
    }

    #[async_std::test]
    async fn overview_patch_rolls_back_tags_on_name_conflict_and_refreshes_registry() {
        let db = Arc::new(Database::init("sqlite::memory:").await.unwrap());
        let instances = Arc::new(InstanceRegistry::new(Arc::new(DbInstanceStore::new(
            db.clone(),
        ))));
        let create = |name: &str, dir: &str| NewInstance {
            name: name.into(),
            dir: dir.into(),
            version: "1.21.1".into(),
            loader: LoaderKind::Vanilla,
            loader_version: None,
            manifest_url: String::new(),
            meta_provider: "prism".into(),
        };
        let one = instances.create(create("First", "first")).await.unwrap();
        instances.create(create("Taken", "taken")).await.unwrap();
        assert!(matches!(
            instances.create(create("Taken", "other-dir")).await,
            Err(InstanceError::NameTaken)
        ));
        let bus = EventBus::new(8);
        let mut events = bus.subscribe();
        let service = OptionsService::new(
            db.clone(),
            instances.clone(),
            Arc::new(SettingsService::new(db.clone())),
            Arc::new(JavaRegistry::new(Arc::new(DbJavaStore::new(db)), 4)),
            bus,
        );

        let patch = |name: &str| OverviewPatch {
            name: Some(name.into()),
            tags: Some("new".into()),
            export_type: None,
        };
        assert!(matches!(
            service.patch_overview(one.id, patch("Taken")).await,
            Err(OptionsError::NameTaken)
        ));
        assert!(service.overview(one.id).await.unwrap().tags.is_empty());
        let view = service
            .patch_overview(one.id, patch("Updated"))
            .await
            .unwrap();
        assert_eq!(view.name, "Updated");
        assert_eq!(view.tags, "new");
        assert_eq!(instances.get(one.id).await.unwrap().name, "Updated");
        assert!(matches!(
            events.try_recv().unwrap(),
            crate::bus::event::CoreEvent::Instance(InstanceEvent::OverviewChanged(id)) if id == one.id
        ));
        assert!(matches!(
            events.try_recv().unwrap(),
            crate::bus::event::CoreEvent::Option(_)
        ));
        service
            .patch_settings(one.id, SettingsPatch::default())
            .await
            .unwrap();
        assert!(matches!(
            events.try_recv().unwrap(),
            crate::bus::event::CoreEvent::Instance(InstanceEvent::SettingsChanged(id)) if id == one.id
        ));
    }
}
