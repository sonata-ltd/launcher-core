use serde::{Serialize, Deserialize};
use ts_rs::TS;

use super::{BaseMessage, WsMessage};


#[derive(Serialize, Deserialize, Debug, Clone)]
#[derive(TS)]
#[ts(export)]
pub struct ScanMessage {
    pub base: BaseMessage,
    pub data: ScanData
}

impl From<ScanMessage> for WsMessage {
    fn from(value: ScanMessage) -> Self {
        WsMessage::Scan(value)
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[derive(TS)]
#[ts(export)]
pub struct ScanData {
    pub integrity: ScanIntegrity,
    pub info: Option<ScanInfo>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[derive(TS)]
#[ts(export)]
pub struct ScanIntegrity {
    pub manifest_path: String,
    pub manifest_exist: bool,
    pub instance_path: String,
    pub instance_exist: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[derive(TS)]
#[ts(export)]
pub struct ScanInfo {
    pub name: String,
    pub version: String,
    pub loader: String
}
