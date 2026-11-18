//! Serializes admission to launch, installation, and content mutations.
use std::{
    collections::HashSet,
    sync::{Arc, Mutex},
};

use super::{
    error::{InstanceError, Result},
    install_jobs::{InstallJobs, Job},
    model::InstanceId,
    running::{Reservation, RunningInstances},
};
use crate::bus::EventBus;

pub(super) struct InstanceActivity {
    pub(super) running: RunningInstances,
    pub(super) install_jobs: InstallJobs,
    lifecycle: Mutex<()>,
    content_jobs: Arc<Mutex<HashSet<InstanceId>>>,
}

impl InstanceActivity {
    pub(super) fn with_tasks(bus: EventBus, tasks: crate::utils::jobs::supervisor::Tasks) -> Self {
        Self {
            running: RunningInstances::with_tasks(bus, tasks),
            install_jobs: Default::default(),
            lifecycle: Default::default(),
            content_jobs: Default::default(),
        }
    }

    fn content_busy(&self, id: InstanceId) -> bool {
        self.content_jobs
            .lock()
            .map(|jobs| jobs.contains(&id))
            .unwrap_or(true)
    }

    pub(super) fn reserve_content_change(&self, id: InstanceId) -> Result<ContentChangeGuard> {
        let _lifecycle = self
            .lifecycle
            .lock()
            .map_err(|_| InstanceError::RunFailed("lifecycle lock poisoned".into()))?;

        if self.running.phase(id).is_some() || self.install_jobs.busy(id) {
            return Err(InstanceError::Busy(id));
        }

        let mut jobs = self
            .content_jobs
            .lock()
            .map_err(|_| InstanceError::RunFailed("content lock poisoned".into()))?;

        if !jobs.insert(id) {
            return Err(InstanceError::Busy(id));
        }

        Ok(ContentChangeGuard {
            id,
            jobs: self.content_jobs.clone(),
        })
    }

    pub(super) fn reserve_install(&self, id: InstanceId) -> Result<Job> {
        let _guard = self
            .lifecycle
            .lock()
            .map_err(|_| InstanceError::CreationFailed("lifecycle lock poisoned".into()))?;

        if self.running.phase(id).is_some() {
            return Err(InstanceError::AlreadyRunning(id));
        }

        if self.content_busy(id) {
            return Err(InstanceError::Busy(id));
        }

        self.install_jobs
            .begin(id)
            .map_err(|_| InstanceError::Busy(id))
    }

    pub(super) fn reserve_launch(&self, id: InstanceId) -> Result<Reservation> {
        let _guard = self
            .lifecycle
            .lock()
            .map_err(|_| InstanceError::RunFailed("lifecycle lock poisoned".into()))?;

        if self.content_busy(id) {
            return Err(InstanceError::Busy(id));
        }
        if self.install_jobs.busy(id) {
            return Err(InstanceError::RunFailed("installation is active".into()));
        }

        self.running.reserve(id)
    }
}

pub(crate) struct ContentChangeGuard {
    id: InstanceId,
    jobs: Arc<Mutex<HashSet<InstanceId>>>,
}

impl Drop for ContentChangeGuard {
    fn drop(&mut self) {
        if let Ok(mut jobs) = self.jobs.lock() {
            jobs.remove(&self.id);
        }
    }
}
