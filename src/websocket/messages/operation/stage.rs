use serde::{Deserialize, Serialize};
use strum::Display;
use ts_rs::TS;


#[derive(Serialize, Deserialize, Debug, Clone, Display)]
#[serde(rename_all = "snake_case")]
#[derive(TS)]
pub enum OperationStage {
    FetchManifest,
    DownloadLibs,
    DownloadAssets,
    VerifyFiles,
    CreateStructure,
    ScanInstances
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[derive(TS)]
pub struct StageResult {
	pub status: StageStatus,
	pub stage: OperationStage,
    pub duration_secs: f64,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<StageError>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "snake_case")]
#[derive(TS)]
pub enum StageStatus {
	Started,
	InProgress,
	Completed,
	Failed,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[derive(TS)]
pub struct StageError {
	// pub code: ErrorCode,

	// TODO: Add fields
}
