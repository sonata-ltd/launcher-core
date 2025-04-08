use serde::{Serialize, Deserialize};
use ts_rs::TS;

use super::{BaseMessage, WsMessage};


#[derive(Serialize, Deserialize, Debug)]
#[derive(TS)]
#[ts(export)]
pub struct ScanMessage<'a> {
    #[serde(flatten, borrow)]
    pub base: BaseMessage<'a>,

    pub data: ScanData<'a>
}

impl<'a> From<ScanMessage<'a>> for WsMessage<'a> {
    fn from(value: ScanMessage<'a>) -> Self {
        WsMessage::Scan(value)
    }
}

#[derive(Serialize, Deserialize, Debug)]
#[derive(TS)]
#[ts(export)]
pub struct ScanData<'a> {
    #[serde(borrow)]
    pub integrity: ScanIntegrity<'a>,
    pub info: Option<ScanInfo>,
}

#[derive(Serialize, Deserialize, Debug)]
#[derive(TS)]
#[ts(export)]
pub struct ScanIntegrity<'a> {
    pub manifest_path: &'a str,
    pub manifest_exist: bool,
    pub instance_path: &'a str,
    pub instance_exist: bool,
}

#[derive(Serialize, Deserialize, Debug)]
#[derive(TS)]
#[ts(export)]
pub struct ScanInfo {
    pub name: String,
    pub version: String,
    pub loader: String
}
