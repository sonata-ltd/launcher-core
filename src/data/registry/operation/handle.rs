use std::sync::{Arc, Mutex};

use crate::{
    bus::EventBus,
    data::registry::operation::message::{
        event::{OperationEvent, OperationFinish, OperationStart, OperationUpdate},
        output::ProcessorOutput,
        stage::{OperationStage, StageResult},
        status::{Outcome, Progress},
        target::ProcessTarget,
        OperationId, OperationMessage,
    },
};

pub struct OperationHandle {
    id: OperationId,
    bus: EventBus,
    records: Arc<Mutex<super::Records>>,
    finished: bool,
}

impl OperationHandle {
    pub fn id(&self) -> OperationId {
        self.id
    }

    pub(super) fn new(id: OperationId, bus: EventBus, records: Arc<Mutex<super::Records>>) -> Self {
        Self {
            id,
            bus,
            records,
            finished: false,
        }
    }

    fn publish(&self, data: impl Into<OperationEvent>) {
        self.bus.publish(OperationMessage {
            operation_id: self.id,
            data: data.into(),
        });
    }

    fn emit(&self, data: impl Into<OperationEvent>) {
        let event = data.into();

        if let Some(snapshot) = self
            .records
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .active
            .get_mut(&self.id)
        {
            snapshot.apply(&event);
        }

        self.bus.publish(OperationMessage {
            operation_id: self.id,
            data: event,
        });
    }

    pub(super) fn start(&self, stages: Vec<OperationStage>) {
        self.emit(OperationStart { stages });
    }

    pub fn start_stage_determinable(&self, stage: OperationStage) {
        self.emit(OperationUpdate::Progress {
            stage,
            status: super::message::status::LifeCycle::Started,
            target: None,
            progress: super::message::status::Progress::Determinable {
                current: 0,
                total: 1,
            },
        });
    }

    pub fn start_stage_indeterminable(&self, stage: OperationStage) {
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

    pub fn skip_stage(&self, stage: OperationStage, reason: Option<String>) {
        self.emit(OperationUpdate::Skipped { stage, reason });
    }

    pub fn output(&self, output: ProcessorOutput) {
        self.publish(OperationUpdate::Output(output));
    }

    pub fn complete_stage(&self, res: StageResult) {
        self.emit(OperationUpdate::Completed(res));
    }

    pub fn finish(mut self, outcome: Outcome) {
        self.close(outcome);
    }

    fn close(&mut self, outcome: Outcome) {
        if std::mem::replace(&mut self.finished, true) {
            return;
        }

        {
            let mut records = self
                .records
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if let Some(mut snapshot) = records.active.remove(&self.id) {
                snapshot.apply(&OperationEvent::Finish(OperationFinish {
                    outcome: outcome.clone(),
                }));
                records.history.push(self.id, (snapshot, outcome.clone()));
            }
        }
        self.publish(OperationFinish { outcome });
    }
}

impl Drop for OperationHandle {
    fn drop(&mut self) {
        self.close(if std::thread::panicking() {
            Outcome::Failed
        } else {
            Outcome::Cancelled
        });
    }
}
