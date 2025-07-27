use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::{process::{ProcessStatus, ProcessTarget}, stage::{OperationStage, StageResult}};


#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "snake_case")]
#[derive(TS)]
#[ts(export)]
pub enum OperationEvent {
    Start(OperationStart),
    Update(OperationUpdate),
    Finish(OperationFinish)
}


#[derive(Serialize, Deserialize, Debug, Clone)]
#[derive(TS)]
#[ts(export)]
pub struct OperationStart {
    pub stages: Vec<OperationStage>,
}

impl<'a> From<OperationStart> for OperationEvent {
    fn from(start: OperationStart) -> Self {
        OperationEvent::Start(start)
    }
}


#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "type", content = "details")]
#[derive(TS)]
#[ts(export)]
pub enum OperationUpdate {
	SingleDeterminable {
		stage: OperationStage,
		status: ProcessStatus,
        target: Option<ProcessTarget>
	},
    Determinable {
		stage: OperationStage,
		status: ProcessStatus,
        target: Option<ProcessTarget>,
        current: usize,
        total: usize
    },
    Indeterminable {
		stage: OperationStage,
        status: ProcessStatus
    },
	Completed(StageResult)
}

impl<'a> From<OperationUpdate> for OperationEvent {
    fn from(msg: OperationUpdate) -> Self {
        OperationEvent::Update(msg)
    }
}


#[derive(Serialize, Deserialize, Debug, Clone)]
#[derive(TS)]
#[ts(export)]
pub struct OperationFinish {
    pub status: OperationStatus,
	// pub error: // TODO: Error
}

impl<'a> From<OperationFinish> for OperationEvent {
    fn from(msg: OperationFinish) -> Self {
        OperationEvent::Finish(msg)
    }
}


#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "snake_case")]
#[derive(TS)]
#[ts(export)]
pub enum OperationStatus {
    Completed,
    Failed,
    Cancelled
}
