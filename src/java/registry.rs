use std::sync::Arc;

use async_std::sync::RwLock;
use linked_hash_map::LinkedHashMap;

use crate::java::{
    error::{JavaError, JavaResult},
    model::{JavaRuntime, NewJavaRuntime},
    storage::JavaStore,
};

pub struct JavaRegistry {
    cache: RwLock<LinkedHashMap<i64, Arc<JavaRuntime>>>,
    limit: usize,
    store: Arc<dyn JavaStore>,
}

impl JavaRegistry {
    pub fn new(store: Arc<dyn JavaStore>, limit: usize) -> Self {
        Self {
            cache: RwLock::new(LinkedHashMap::new()),
            limit,
            store,
        }
    }

    pub async fn register(&self, new: NewJavaRuntime) -> JavaResult<Arc<JavaRuntime>> {
        if self.store.exists_by_path(&new.exec_path).await? {
            return Err(JavaError::AlreadyRegistered(
                new.exec_path.display().to_string(),
            ));
        }

        let java = self.store.insert(&new).await?;

        Ok(self.upsert(java).await)
    }

    pub async fn get(&self, id: i64) -> JavaResult<Option<Arc<JavaRuntime>>> {
        {
            let cache = self.cache.read().await;

            if let Some(java) = cache.get(&id) {
                return Ok(Some(Arc::clone(java)));
            }
        }

        let Some(java) = self.store.get_by_id(id).await? else {
            return Ok(None);
        };

        Ok(Some(self.upsert(java).await))
    }

    async fn upsert(&self, java: JavaRuntime) -> Arc<JavaRuntime> {
        let id = java.id;
        let java = Arc::new(java);

        let mut cache = self.cache.write().await;

        if let Some(existing) = cache.get(&id) {
            return Arc::clone(existing);
        }

        cache.insert(id, Arc::clone(&java));

        while cache.len() > self.limit {
            cache.pop_front();
        }

        java
    }
}
