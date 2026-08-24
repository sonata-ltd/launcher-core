use serde::{Deserialize, Serialize};

use crate::data::registry::operation::message::{
    stage::{OperationStage, StageResult},
    status::{LifeCycle, Outcome, Progress},
    target::ProcessTarget,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum OperationEvent {
    Start(OperationStart),
    Update(OperationUpdate),
    Finish(OperationFinish),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperationStart {
    pub stages: Vec<OperationStage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "details")]
pub enum OperationUpdate {
    Progress {
        stage: OperationStage,
        status: LifeCycle,
        target: Option<ProcessTarget>,
        progress: Progress,
    },
    Completed(StageResult),
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct OperationFinish {
    pub outcome: Outcome,
    // pub error: // TODO: Error
}

impl From<OperationStart> for OperationEvent {
    fn from(value: OperationStart) -> Self {
        OperationEvent::Start(value)
    }
}

impl From<OperationUpdate> for OperationEvent {
    fn from(value: OperationUpdate) -> Self {
        OperationEvent::Update(value)
    }
}

impl From<OperationFinish> for OperationEvent {
    fn from(value: OperationFinish) -> Self {
        OperationEvent::Finish(value)
    }
}
