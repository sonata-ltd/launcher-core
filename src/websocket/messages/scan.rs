use serde::{Serialize, Deserialize};
use ts_rs::TS;

use super::{BaseMessage, WsMessage};


#[derive(Serialize, Deserialize, Debug, Clone)]
#[derive(TS)]
pub struct ScanMessage {
    pub base: BaseMessage,
    pub data: ScanData
}

impl<'a> From<ScanMessage> for WsMessage<'a> {
    fn from(value: ScanMessage) -> Self {
        WsMessage::Scan(value)
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[derive(TS)]
pub struct ScanData {
    pub integrity: ScanIntegrity,
    pub info: Option<ScanInfo>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[derive(TS)]
pub struct ScanIntegrity {
    pub instance_path: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[derive(TS)]
pub struct ScanInfo {
    pub id: i64,
    pub name: String,
    pub version: String,
    pub loader: String
}
