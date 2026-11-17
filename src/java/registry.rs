use std::{path::PathBuf, sync::Arc};

use async_std::sync::Mutex;
use linked_hash_map::LinkedHashMap;

use crate::data::db::DbError;
use crate::java::{
    error::{JavaError, JavaResult},
    model::{JavaRuntime, NewJavaRuntime},
    storage::JavaStore,
};

pub struct JavaRegistry {
    cache: Mutex<LinkedHashMap<i64, Arc<JavaRuntime>>>,
    limit: usize,
    store: Arc<dyn JavaStore>,
}

impl JavaRegistry {
    pub fn new(store: Arc<dyn JavaStore>, limit: usize) -> Self {
        Self {
            cache: Mutex::new(LinkedHashMap::new()),
            limit,
            store,
        }
    }

    pub async fn register(&self, new: NewJavaRuntime) -> JavaResult<Arc<JavaRuntime>> {
        let Some(exec_path) = &new.exec_path else {
            return Err(DbError::InsufficientData("exec_path is None".into()).into());
        };

        let mut cache = self.cache.lock().await;
        if self.store.exists_by_path(exec_path).await? {
            return Err(JavaError::AlreadyRegistered(
                exec_path.display().to_string(),
            ));
        }

        let java = self.store.insert(&new).await?;

        Ok(self.upsert(&mut cache, java))
    }

    /// Registers every runtime whose `exec_path` is new; returns those.
    pub async fn register_many(
        &self,
        new: Vec<NewJavaRuntime>,
    ) -> JavaResult<Vec<Arc<JavaRuntime>>> {
        let mut cache = self.cache.lock().await;
        let added = self.store.insert_many(&new).await?;
        let mut out = Vec::with_capacity(added.len());

        for java in added {
            out.push(self.upsert(&mut cache, java));
        }

        Ok(out)
    }

    pub async fn backfill_major(&self) -> JavaResult<()> {
        let mut cache = self.cache.lock().await;
        cache.clear();
        Ok(self.store.backfill_major().await?)
    }

    pub async fn list_all(&self) -> JavaResult<Vec<Arc<JavaRuntime>>> {
        let mut cache = self.cache.lock().await;
        let all = self.store.list_all().await?;
        cache.clear();
        let mut out = Vec::with_capacity(all.len());

        for java in all {
            out.push(self.upsert(&mut cache, java));
        }

        Ok(out)
    }

    pub async fn get(&self, id: i64) -> JavaResult<Option<Arc<JavaRuntime>>> {
        let mut cache = self.cache.lock().await;
        if let Some(java) = cache.get_refresh(&id) {
            return Ok(Some(Arc::clone(java)));
        }

        let Some(java) = self.store.get_by_id(id).await? else {
            return Ok(None);
        };

        Ok(Some(self.upsert(&mut cache, java)))
    }

    pub async fn exists_by_exec_path(&self, path: &PathBuf) -> JavaResult<bool> {
        Ok(self.store.exists_by_path(path).await?)
    }

    fn upsert(
        &self,
        cache: &mut LinkedHashMap<i64, Arc<JavaRuntime>>,
        java: JavaRuntime,
    ) -> Arc<JavaRuntime> {
        let java = Arc::new(java);
        cache.insert(java.id, Arc::clone(&java));
        while cache.len() > self.limit {
            cache.pop_front();
        }

        java
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{data::db::Database, java::storage::DbJavaStore};

    #[async_std::test]
    async fn registry_invalidates_backfill_results_and_evicts_the_least_recently_used_entry() {
        let db = Arc::new(Database::init("sqlite::memory:").await.unwrap());
        for id in 1..=3i64 {
            sqlx::query("INSERT INTO java_runtimes(id,version,exec_path,home_path) VALUES(?,'21.0.5',?,'/')")
                .bind(id).bind(format!("/java-{id}")).execute(&db.pool).await.unwrap();
        }
        let registry = JavaRegistry::new(Arc::new(DbJavaStore::new(db)), 2);
        assert_eq!(registry.get(1).await.unwrap().unwrap().major, 0);
        registry.backfill_major().await.unwrap();
        assert_eq!(registry.get(1).await.unwrap().unwrap().major, 21);
        registry.get(2).await.unwrap();
        registry.get(1).await.unwrap();
        registry.get(3).await.unwrap();
        let keys: Vec<_> = registry.cache.lock().await.keys().copied().collect();
        assert_eq!(keys, [1, 3]);
    }
}
