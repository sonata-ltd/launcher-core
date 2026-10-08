use std::{path::PathBuf, sync::Arc};

use crate::{
    bus::EventBus,
    data::db::Database,
    java::{
        error::JavaResult,
        model::{JavaEvent, JavaRuntime, NewJavaRuntime},
        registry::JavaRegistry,
        storage::{DbJavaStore, JavaStore},
    },
};

pub const DEFAULT_JAVAREG_CACHE_SIZE: usize = 100;

pub struct JavaService {
    registry: Arc<JavaRegistry>,
    bus: EventBus,
}

impl JavaService {
    pub fn new(db: Arc<Database>, bus: EventBus) -> Self {
        let store: Arc<dyn JavaStore> = Arc::new(DbJavaStore::new(db));

        Self {
            registry: Arc::new(JavaRegistry::new(store, DEFAULT_JAVAREG_CACHE_SIZE)),
            bus,
        }
    }

    pub(crate) fn get_registry(&self) -> Arc<JavaRegistry> {
        Arc::clone(&self.registry)
    }

    pub async fn list_all(&self) -> JavaResult<Vec<Arc<JavaRuntime>>> {
        self.registry.list_all().await
    }

    pub async fn is_external_already_registerd(&self, exec_path: &PathBuf) -> JavaResult<bool> {
        Ok(self.registry.exists_by_exec_path(exec_path).await?)
    }

    pub async fn init_external(&self, new_java: NewJavaRuntime) -> JavaResult<Arc<JavaRuntime>> {
        let res = self.registry.register(new_java).await;

        match res {
            Ok(rec) => {
                self.bus.publish(JavaEvent::ListChanged);
                Ok(rec)
            }
            Err(e) => Err(e),
        }
    }
}
