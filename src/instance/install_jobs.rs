use super::model::InstanceId;
use crate::utils::download::transfer::Cancel;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

#[derive(Debug, thiserror::Error)]
pub(crate) enum InstallJobError {
    #[error("installation lock poisoned")]
    Unavailable,
    #[error("another installation is active")]
    Busy,
}

#[derive(Clone, Default)]
pub(crate) struct InstallJobs {
    active: Arc<Mutex<HashMap<InstanceId, Cancel>>>,
}

pub struct Job {
    id: InstanceId,
    jobs: InstallJobs,
    pub cancel: Cancel,
}

impl InstallJobs {
    pub fn begin(&self, id: InstanceId) -> Result<Job, InstallJobError> {
        let mut jobs = self
            .active
            .lock()
            .map_err(|_| InstallJobError::Unavailable)?;

        // Deliberately serialize installations until shared legacy assets sync is cancellable.
        if !jobs.is_empty() {
            return Err(InstallJobError::Busy);
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
