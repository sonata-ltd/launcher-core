use event::OperationEvent;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::{BaseMessage, WsMessage};

pub mod event;
pub mod stage;
pub mod process;
pub mod progress;


#[derive(Serialize, Deserialize, Debug, Clone)]
#[derive(TS)]
#[ts(export)]
pub struct OperationMessage {
    pub base: BaseMessage,
    pub data: OperationEvent
}

impl From<OperationMessage> for WsMessage {
    fn from(msg: OperationMessage) -> Self {
        WsMessage::Operation(msg)
    }
}


#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "snake_case")]
#[derive(TS)]
#[ts(export)]
pub enum RequestedTask {
    InitInstance {
        instance_id: String,
        config: String,
    },
    RunInstance {
        instance_id: String,
        parameters: Vec<String>,
    },
    ScanForInstances {
        scan_range: (u32, u32),
    },
}
