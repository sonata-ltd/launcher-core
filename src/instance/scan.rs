use crate::instance::model::InstanceId;

#[derive(Debug, Clone)]
pub struct ScanMessage {
    pub data: ScanData,
}

#[derive(Debug, Clone)]
pub struct ScanData {
    pub integrity: ScanIntegrity,
    pub info: Option<ScanInfo>,
}

#[derive(Debug, Clone)]
pub struct ScanIntegrity {
    pub instance_path: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ScanInfo {
    pub id: InstanceId,
    pub name: String,
    pub version: String,
    pub loader: String,
}
