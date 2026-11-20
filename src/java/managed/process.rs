//! Broad managed-Java retention precedes SQL/runtime selection. This capability
//! retains admission while preparing; the durable pin survives caller/process
//! drop. Children receive independent pins; broad completion follows all owned
//! reads/work and cannot acknowledge an unconfirmed child exit.
use crate::{
    data::db::Database,
    utils::{
        cancellation::Cancellation,
        object_store::{ObjectStore, StoreAccess},
        process_pin::ProcessPin,
    },
};
use std::{
    io,
    path::{Path, PathBuf},
    sync::Arc,
};

#[derive(Clone)]
pub(crate) struct JavaRetention {
    database: Arc<Database>,
    root: PathBuf,
}

impl JavaRetention {
    pub(crate) fn new(database: Arc<Database>, root: PathBuf) -> Self {
        Self { database, root }
    }

    pub(crate) async fn begin(&self, cancel: &Cancellation) -> io::Result<JavaUse> {
        cancel.check()?;
        super::retention::attach(Some(self.database.clone()), self.root.clone())
            .await
            .map_err(io::Error::other)?;
        let access = ObjectStore::beneath(&self.root, Path::new(".managed"))?
            .access(cancel)
            .await?;
        JavaUse::admitted(access)
    }
}

#[derive(Clone)]
pub(crate) struct JavaUse {
    pin: ProcessPin,
    _access: StoreAccess,
}

impl JavaUse {
    pub(super) fn admitted(access: StoreAccess) -> io::Result<Self> {
        let pin = ProcessPin::create_in(access.directory())?;
        Ok(Self {
            pin,
            _access: access,
        })
    }
    #[cfg(test)]
    pub(crate) fn pin(&self) -> ProcessPin {
        self.pin.clone()
    }
    /// Independent child pins must not release the broad operation pin or be
    /// acknowledged by its completion after an unconfirmed spawn/exit.
    pub(crate) fn child_pin(&self) -> io::Result<ProcessPin> {
        ProcessPin::create_in(self._access.directory())
    }
    pub(crate) fn finish(self, process_exited: bool) {
        self.pin.finish(process_exited);
    }
}

#[cfg(test)]
mod tests;
