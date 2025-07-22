use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::operation::stage::OperationStage;

#[derive(Serialize, Deserialize, Debug)]
#[derive(TS)]
#[ts(export)]
pub struct Task<'a> {
    pub name: &'a str,
    pub status: TaskStatus,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub stage: Option<OperationStage>,
    pub progress: TaskProgress,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<&'a str>
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(tag = "type", content = "details")]
#[derive(TS)]
#[ts(export)]
pub enum TaskProgress {
    Determinable {
        current: Option<usize>,
        total: Option<usize>
    },
    Indeterminable
}

#[derive(Serialize, Deserialize, Debug)]
#[derive(TS)]
#[ts(export)]
pub enum TaskStatus {
    Pending,
    Running,
    Completed,
    Failed,
    CancelledAwaiting,
    Cancelled
}
