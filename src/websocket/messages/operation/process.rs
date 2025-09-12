use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::websocket::messages::scan::{ScanInfo, ScanIntegrity};

use super::progress::ProgressUnit;

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "snake_case")]
#[derive(TS)]
pub enum ProcessStatus {
    Started,
    InProgress,
    Completed,
    Failed,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "type")]
#[derive(TS)]
pub enum ProcessTarget {
    File {
        status: TargetStatus,
        name: String,

        #[serde(skip_serializing_if = "Option::is_none")]
        unit: Option<ProgressUnit>,

        #[serde(skip_serializing_if = "Option::is_none")]
        current: Option<usize>,

        #[serde(skip_serializing_if = "Option::is_none")]
        size: Option<usize>,
    },
    Dir {
        // TODO
    },
    Instance {
        integrity: ScanIntegrity,
        info: Option<ScanInfo>,
    },
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "snake_case")]
#[derive(TS)]
pub enum TargetStatus {
    File(FileStatus),
    Dir(DirStatus),
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "snake_case")]
#[derive(TS)]
pub enum FileStatus {
    Downloading,
    Downloaded,
    FailedToDownload,
}

impl From<FileStatus> for TargetStatus {
    fn from(status: FileStatus) -> Self {
        TargetStatus::File(status)
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "snake_case")]
#[derive(TS)]
pub enum DirStatus {
    Created,
    FailedToCreate,
}

impl ProcessTarget {
    pub fn file(name: String, status: FileStatus) -> Self {
        ProcessTarget::File {
            status: status.into(),
            name,
            unit: None,
            current: None,
            size: None,
        }
    }

    #[allow(dead_code)]
    pub fn file_with_details(
        name: String,
        status: FileStatus,
        unit: Option<ProgressUnit>,
        current: Option<usize>,
        size: Option<usize>,
    ) -> Self {
        ProcessTarget::File {
            status: status.into(),
            name,
            unit,
            current,
            size,
        }
    }

    pub fn instance(
        instance_path: Option<String>,
        scan_info: Option<ScanInfo>,
    ) -> Self {
        ProcessTarget::Instance {
            integrity: ScanIntegrity {
                instance_path,
            },
            info: scan_info,
        }
    }

    // TODO for Dir
}
