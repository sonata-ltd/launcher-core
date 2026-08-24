use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicU32, Ordering},
        Arc, Mutex,
    },
};

use crate::{
    bus::EventBus,
    data::registry::operation::{
        handle::OperationHandle,
        message::{
            stage::OperationStage,
            status::{LifeCycle, Progress},
            OperationId,
        },
        snapshot::OperationSnapshot,
    },
};

pub mod handle;
pub mod message;
pub mod snapshot;

pub struct OperationRegistry {
    active: Arc<Mutex<HashMap<OperationId, OperationSnapshot>>>,
    bus: EventBus,
    next_id: AtomicU32,
}

impl OperationRegistry {
    pub fn new(bus: EventBus) -> Self {
        Self {
            active: Arc::new(Mutex::new(HashMap::new())),
            bus,
            next_id: AtomicU32::new(0),
        }
    }

    pub fn begin(&self, name: String, stages: Vec<OperationStage>) -> OperationHandle {
        let id = OperationId::new(self.next_id.fetch_add(1, Ordering::Relaxed));

        let snapshot = OperationSnapshot {
            id,
            name,
            stage: None,
            status: LifeCycle::Started,
            progress: Progress::Indeterminable,
            message: None,
        };

        if let Ok(mut active) = self.active.lock() {
            active.insert(id, snapshot);
        }

        let handle = OperationHandle::new(id, self.bus.clone(), Arc::clone(&self.active));
        handle.start(stages);
        handle
    }

    pub fn snapshot_all(&self) -> Vec<OperationSnapshot> {
        self.active
            .lock()
            .map(|a| a.values().cloned().collect())
            .unwrap_or_default()
    }
}
