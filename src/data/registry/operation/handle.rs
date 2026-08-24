use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use crate::{
    bus::EventBus,
    data::registry::operation::{
        message::{
            event::{OperationEvent, OperationFinish, OperationStart, OperationUpdate},
            stage::{OperationStage, StageResult},
            status::{Outcome, Progress},
            target::ProcessTarget,
            OperationId, OperationMessage,
        },
        snapshot::OperationSnapshot,
    },
};

pub struct OperationHandle {
    id: OperationId,
    bus: EventBus,
    snapshots: Arc<Mutex<HashMap<OperationId, OperationSnapshot>>>,
}

impl OperationHandle {
    pub fn new(
        id: OperationId,
        bus: EventBus,
        snapshots: Arc<Mutex<HashMap<OperationId, OperationSnapshot>>>,
    ) -> Self {
        Self { id, bus, snapshots }
    }

    fn publish(&self, data: impl Into<OperationEvent>) {
        self.bus.publish(OperationMessage {
            operation_id: self.id,
            data: data.into(),
        });
    }

    fn emit(&self, data: impl Into<OperationEvent>) {
        let event = data.into();

        if let Ok(mut active) = self.snapshots.lock() {
            if let Some(snapshot) = active.get_mut(&self.id) {
                snapshot.apply(&event);
            }
        }

        self.bus.publish(OperationMessage {
            operation_id: self.id,
            data: event,
        });
    }

    pub(super) fn start(&self, stages: Vec<OperationStage>) {
        self.emit(OperationStart { stages });
    }

    pub fn start_stage(&self, stage: OperationStage) {
        self.emit(OperationUpdate::Progress {
            stage,
            status: super::message::status::LifeCycle::Started,
            target: None,
            progress: super::message::status::Progress::Indeterminable,
        });
    }

    pub fn update_stage(
        &self,
        stage: OperationStage,
        current: usize,
        total: usize,
        target: Option<ProcessTarget>,
    ) {
        self.emit(OperationUpdate::Progress {
            stage,
            status: super::message::status::LifeCycle::InProgress,
            target,
            progress: Progress::Determinable { current, total },
        });
    }

    pub fn complete_stage(&self, res: StageResult) {
        self.emit(OperationUpdate::Completed(res));
    }

    pub fn finish(self, outcome: Outcome) {
        self.publish(OperationFinish { outcome });

        if let Ok(mut active) = self.snapshots.lock() {
            active.remove(&self.id);
        }
    }
}
