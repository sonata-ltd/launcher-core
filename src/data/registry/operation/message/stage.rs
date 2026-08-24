use serde::{Deserialize, Serialize};
use strum::Display;
use ts_rs::TS;

use crate::data::registry::operation::message::status::Outcome;

#[derive(Debug, Clone, Serialize, Deserialize, Display)]
pub enum OperationStage {
    FetchManifest,
    DownloadLibs,
    DownloadAssets,
    VerifyFiles,
    CreateStructure,
    ScanInstances,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StageResult {
    pub status: Outcome,
    pub stage: OperationStage,
    pub duration_secs: f64,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<StageError>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct StageError {
    // pub code: ErrorCode,

    // TODO: Add fields
}
