use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use crate::{
    data::db::DbError,
    instance::{
        error::InstanceError,
        model::{InstanceId, InstanceRecord, NewInstance},
        storage::InstanceStore,
    },
};

use crate::instance::Result;

pub struct InstanceRegistry {
    cache: Mutex<HashMap<InstanceId, Arc<InstanceRecord>>>,
    store: Arc<dyn InstanceStore>,
    updates: async_std::sync::Mutex<()>,
}

impl InstanceRegistry {
    pub fn new(store: Arc<dyn InstanceStore>) -> Self {
        Self {
            cache: Mutex::new(HashMap::new()),
            store,
            updates: Default::default(),
        }
    }

    pub async fn create(&self, new: NewInstance) -> Result<Arc<InstanceRecord>> {
        let _updates = self.lock_updates().await;
        if self.store.exists_by_dir(&new.dir).await? {
            return Err(InstanceError::NameTaken);
        }

        let rec = self.store.insert(&new).await.map_err(map_name_conflict)?;

        let new_rec = Arc::new(rec);
        self.cache
            .lock()
            .unwrap()
            .insert(new_rec.id, Arc::clone(&new_rec));

        Ok(new_rec)
    }

    pub async fn get(&self, id: InstanceId) -> Result<Arc<InstanceRecord>> {
        let _updates = self.lock_updates().await;
        self.get_locked(id).await
    }

    async fn get_locked(&self, id: InstanceId) -> Result<Arc<InstanceRecord>> {
        {
            let cache = self.cache.lock().unwrap();

            if let Some(rec) = cache.get(&id) {
                return Ok(rec.clone());
            }
        }

        let rec = self
            .store
            .get_by_id(id)
            .await?
            .ok_or(InstanceError::NotFound(id))?;

        Ok(self.upsert(rec))
    }

    pub async fn list(&self) -> Result<Vec<Arc<InstanceRecord>>> {
        let _updates = self.lock_updates().await;
        let rows = self.store.list_all().await?;

        let mut out = Vec::with_capacity(rows.len());
        let mut cache = self.cache.lock().unwrap();
        cache.clear();

        for rec in rows {
            let rec = Arc::new(rec);
            cache.insert(rec.id, Arc::clone(&rec));
            out.push(rec);
        }

        Ok(out)
    }

    pub async fn rename(&self, id: InstanceId, new_name: &str) -> Result<Arc<InstanceRecord>> {
        let _updates = self.lock_updates().await;
        let current = self.get_locked(id).await?;

        if current.name == new_name {
            return Ok(current);
        }

        self.invalidate(id);
        self.store
            .update_name(id, new_name)
            .await
            .map_err(map_name_conflict)?;

        let mut updated = (*current).clone();
        updated.name = new_name.to_string();

        Ok(self.upsert(updated))
    }

    pub fn all(&self) -> Vec<Arc<InstanceRecord>> {
        let cache = self.cache.lock().unwrap();
        cache.values().cloned().collect()
    }

    pub async fn delete(&self, id: InstanceId) -> Result<()> {
        let _updates = self.lock_updates().await;
        self.store.delete(id).await?;
        self.invalidate(id);

        Ok(())
    }

    pub(super) async fn lock_updates(&self) -> async_std::sync::MutexGuard<'_, ()> {
        self.updates.lock().await
    }

    pub(super) fn invalidate(&self, id: InstanceId) {
        self.cache.lock().unwrap().remove(&id);
    }

    fn upsert(&self, record: InstanceRecord) -> Arc<InstanceRecord> {
        let record = Arc::new(record);
        self.cache
            .lock()
            .unwrap()
            .insert(record.id, Arc::clone(&record));
        record
    }
}

fn map_name_conflict(error: DbError) -> InstanceError {
    match error {
        DbError::Conflict(_) => InstanceError::NameTaken,
        other => InstanceError::DB(other),
    }
}
