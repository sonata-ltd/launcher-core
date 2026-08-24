use std::sync::Arc;

use crate::{
    data::db::Database,
    java::{
        error::JavaResult,
        model::{JavaRuntime, NewJavaRuntime},
        registry::JavaRegistry,
        storage::{DbJavaStore, JavaStore},
    },
};

pub const DEFAULT_JAVAREG_CACHE_SIZE: usize = 100;

pub struct JavaService {
    registry: Arc<JavaRegistry>,
}

impl JavaService {
    pub fn new(db: Arc<Database>) -> Self {
        let store: Arc<dyn JavaStore> = Arc::new(DbJavaStore::new(db));

        Self {
            registry: Arc::new(JavaRegistry::new(store, DEFAULT_JAVAREG_CACHE_SIZE)),
        }
    }

    pub(crate) fn get_registry(&self) -> Arc<JavaRegistry> {
        Arc::clone(&self.registry)
    }

    pub async fn init_external(&self, new_java: NewJavaRuntime) -> JavaResult<Arc<JavaRuntime>> {
        self.registry.register(new_java).await
    }
}
