use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::{process::{ProcessStatus, ProcessTarget}, stage::{OperationStage, StageResult}};


#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "snake_case")]
#[derive(TS)]
#[ts(export)]
pub enum OperationEvent<'a> {
    Start(OperationStart),
    #[serde(borrow)]
    Update(OperationUpdate<'a>),
    Finish(OperationFinish)
}


#[derive(Serialize, Deserialize, Debug)]
#[derive(TS)]
#[ts(export)]
pub struct OperationStart {
    pub stages: Vec<OperationStage>,
}

impl<'a> From<OperationStart> for OperationEvent<'a> {
    fn from(start: OperationStart) -> Self {
        OperationEvent::Start(start)
    }
}


#[derive(Serialize, Deserialize, Debug)]
#[serde(tag = "type", content = "details")]
#[derive(TS)]
#[ts(export)]
pub enum OperationUpdate<'a> {
	SingleDeterminable {
		stage: OperationStage,
		status: ProcessStatus,
		#[serde(borrow)]
        target: Option<ProcessTarget<'a>>
	},
    Determinable {
		stage: OperationStage,
		status: ProcessStatus,
		#[serde(borrow)]
        target: Option<ProcessTarget<'a>>,
        current: usize,
        total: usize
    },
    Indeterminable {
		stage: OperationStage,
        status: ProcessStatus
    },
	Completed(StageResult)
}

impl<'a> From<OperationUpdate<'a>> for OperationEvent<'a> {
    fn from(msg: OperationUpdate<'a>) -> Self {
        OperationEvent::Update(msg)
    }
}


#[derive(Serialize, Deserialize, Debug)]
#[derive(TS)]
#[ts(export)]
pub struct OperationFinish {
    pub status: OperationStatus,
	// pub error: // TODO: Error
}

impl<'a> From<OperationFinish> for OperationEvent<'a> {
    fn from(msg: OperationFinish) -> Self {
        OperationEvent::Finish(msg)
    }
}


#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "snake_case")]
#[derive(TS)]
#[ts(export)]
pub enum OperationStatus {
    Completed,
    Failed,
    Cancelled
}
