//! Compatibility API for installation reservations. Instance lifecycle code owns the registry.
use super::error::{ModloaderError, Result};
use crate::instance::{install_jobs, model::InstanceId};

pub use install_jobs::Job;

#[derive(Clone, Default)]
pub struct InstallJobs(install_jobs::InstallJobs);

impl InstallJobs {
    pub fn begin(&self, id: InstanceId) -> Result<Job> {
        self.0
            .begin(id)
            .map_err(|error| ModloaderError::Invalid(error.to_string()))
    }

    pub fn cancel(&self, id: InstanceId) {
        self.0.cancel(id);
    }

    pub fn busy(&self, id: InstanceId) -> bool {
        self.0.busy(id)
    }
}
