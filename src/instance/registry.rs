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
}

impl InstanceRegistry {
    pub fn new(store: Arc<dyn InstanceStore>) -> Self {
        Self {
            cache: Mutex::new(HashMap::new()),
            store,
        }
    }

    pub async fn create(&self, new: NewInstance) -> Result<Arc<InstanceRecord>> {
        if self.store.exists_by_dir(&new.dir).await? {
            return Err(InstanceError::NameTaken);
        }

        let rec = self.store.insert(&new).await?;

        let new_rec = Arc::new(rec);
        self.cache
            .lock()
            .unwrap()
            .insert(new_rec.id, Arc::clone(&new_rec));

        Ok(new_rec)
    }

    pub async fn get(&self, id: InstanceId) -> Result<Arc<InstanceRecord>> {
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
        let rows = self.store.list_all().await?;

        let mut out = Vec::with_capacity(rows.len());
        let mut cache = self.cache.lock().unwrap();

        for rec in rows {
            let rec = Arc::new(rec);
            cache.insert(rec.id, Arc::clone(&rec));
            out.push(rec);
        }

        Ok(out)
    }

    pub async fn rename(&self, id: InstanceId, new_name: &str) -> Result<Arc<InstanceRecord>> {
        let current = self.get(id).await?;

        if current.name == new_name {
            return Ok(current);
        }

        self.store
            .update_name(id, new_name)
            .await
            .map_err(|e| match e {
                DbError::Conflict(_) => InstanceError::NameTaken,
                other => InstanceError::DB(other),
            })?;

        let mut updated = (*current).clone();
        updated.name = new_name.to_string();

        Ok(self.upsert(updated))
    }

    pub fn all(&self) -> Vec<Arc<InstanceRecord>> {
        let cache = self.cache.lock().unwrap();
        cache.values().cloned().collect()
    }

    pub async fn delete(&self, id: InstanceId) -> Result<()> {
        self.store.delete(id).await?;
        let _ = self.cache.lock().unwrap().remove(&id);

        Ok(())
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
