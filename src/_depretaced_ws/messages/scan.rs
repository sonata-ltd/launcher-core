use serde::{Serialize, Deserialize};

use super::base::BaseMessage;


#[derive(Serialize, Deserialize, Debug)]
pub struct ScanMessage<'a> {
    #[serde(flatten, borrow)]
    pub base: BaseMessage<'a>,

    pub target: ScanData
}

#[derive(Serialize, Deserialize, Debug)]
pub struct ScanData {
    pub integrity: ScanIntegrity,
    pub info: Option<ScanInfo>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct ScanIntegrity {
    pub manifest_path: String,
    pub manifest_exist: bool,
    pub instance_path: String,
    pub instance_exist: bool,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct ScanInfo {
    pub name: String,
    pub version: String,
    pub loader: String
}
