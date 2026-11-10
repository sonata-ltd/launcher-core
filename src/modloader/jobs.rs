use super::{
    error::{ModloaderError, Result},
    transfer::Cancel,
};
use crate::instance::model::InstanceId;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

#[derive(Clone, Default)]
pub struct InstallJobs {
    active: Arc<Mutex<HashMap<InstanceId, Cancel>>>,
}
pub struct Job {
    id: InstanceId,
    jobs: InstallJobs,
    pub cancel: Cancel,
}
impl InstallJobs {
    pub fn begin(&self, id: InstanceId) -> Result<Job> {
        let mut jobs = self
            .active
            .lock()
            .map_err(|_| ModloaderError::Invalid("installation lock poisoned".into()))?;
        // Deliberately serialize installations until shared legacy assets sync is cancellable.
        if !jobs.is_empty() {
            return Err(ModloaderError::Invalid(
                "another installation is active".into(),
            ));
        }
        let cancel = Cancel::default();
        jobs.insert(id, cancel.clone());
        Ok(Job {
            id,
            jobs: self.clone(),
            cancel,
        })
    }
    pub fn cancel(&self, id: InstanceId) {
        if let Ok(jobs) = self.active.lock() {
            if let Some(cancel) = jobs.get(&id) {
                cancel.cancel();
            }
        }
    }
    pub fn busy(&self, id: InstanceId) -> bool {
        self.active
            .lock()
            .map(|jobs| jobs.contains_key(&id))
            .unwrap_or(true)
    }
}
impl Drop for Job {
    fn drop(&mut self) {
        if let Ok(mut jobs) = self.jobs.active.lock() {
            jobs.remove(&self.id);
        }
    }
}
