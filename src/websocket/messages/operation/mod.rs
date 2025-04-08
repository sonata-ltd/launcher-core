use event::OperationEvent;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::{BaseMessage, WsMessage};

pub mod event;
pub mod stage;
pub mod process;
pub mod progress;


#[derive(Serialize, Deserialize, Debug)]
#[derive(TS)]
#[ts(export)]
pub struct OperationMessage<'a> {
    #[serde(flatten, borrow)]
    pub base: BaseMessage<'a>,
    pub data: OperationEvent<'a>
}

impl<'a> From<OperationMessage<'a>> for WsMessage<'a> {
    fn from(msg: OperationMessage<'a>) -> Self {
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
