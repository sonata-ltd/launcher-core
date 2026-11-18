use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicU32, Ordering},
        Arc, Mutex,
    },
    time::Duration,
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

use crate::utils::jobs::history::History;
use message::status::Outcome;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct OperationResult {
    pub snapshot: OperationSnapshot,
    pub outcome: Outcome,
    pub finished_at: chrono::DateTime<chrono::Utc>,
}

/// An atomic view for initial subscription and event-overflow reconciliation.
/// History is bounded in memory, not a durable journal or a retention pin.
#[derive(Debug, Clone)]
pub struct OperationReconciliation {
    pub active: Vec<OperationSnapshot>,
    pub completed: Vec<OperationResult>,
}

pub(super) struct Records {
    active: HashMap<OperationId, OperationSnapshot>,
    history: History<OperationId, (OperationSnapshot, Outcome)>,
}

pub struct OperationRegistry {
    pub(crate) tasks: crate::utils::jobs::supervisor::Tasks,
    records: Arc<Mutex<Records>>,
    bus: EventBus,
    next_id: AtomicU32,
}

impl OperationRegistry {
    pub fn new(bus: EventBus) -> Self {
        Self::with_history(bus, 128, Duration::from_secs(3600))
    }

    pub fn with_history(bus: EventBus, capacity: usize, ttl: Duration) -> Self {
        Self {
            tasks: Default::default(),
            records: Arc::new(Mutex::new(Records {
                active: HashMap::new(),
                history: History::new(capacity, ttl),
            })),
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
            stages: Vec::new(),
            status: LifeCycle::Started,
            progress: Progress::Indeterminable,
            message: None,
        };

        self.records
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .active
            .insert(id, snapshot);

        let handle = OperationHandle::new(id, self.bus.clone(), Arc::clone(&self.records));
        handle.start(stages);
        handle
    }

    pub fn snapshot_all(&self) -> Vec<OperationSnapshot> {
        self.reconcile().active
    }

    pub fn reconcile(&self) -> OperationReconciliation {
        let mut records = self
            .records
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        records.history.prune();
        OperationReconciliation {
            active: records.active.values().cloned().collect(),
            completed: records
                .history
                .records()
                .map(|record| OperationResult {
                    snapshot: record.value.0.clone(),
                    outcome: record.value.1.clone(),
                    finished_at: record.finished_at,
                })
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests;
