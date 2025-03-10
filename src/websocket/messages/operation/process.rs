use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::websocket::messages::scan::{ScanInfo, ScanIntegrity};

use super::progress::ProgressUnit;

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "snake_case")]
#[derive(TS)]
#[ts(export)]
pub enum ProcessStatus {
    Started,
    InProgress,
    Completed,
    Failed,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(tag = "type")]
#[derive(TS)]
#[ts(export)]
pub enum ProcessTarget<'a> {
    File {
        status: TargetStatus,
        name: &'a str,

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
        integrity: ScanIntegrity<'a>,
        info: Option<ScanInfo>,
    },
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "snake_case")]
#[derive(TS)]
#[ts(export)]
pub enum TargetStatus {
    File(FileStatus),
    Dir(DirStatus),
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "snake_case")]
#[derive(TS)]
#[ts(export)]
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

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "snake_case")]
#[derive(TS)]
#[ts(export)]
pub enum DirStatus {
    Created,
    FailedToCreate,
}

impl<'a> ProcessTarget<'a> {
    pub fn file(name: &'a str, status: FileStatus) -> Self {
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
        name: &'a str,
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
        manifest_path: &'a str,
        manifest_exist: bool,
        instance_path: &'a str,
        instance_exist: bool,
        scan_info: Option<ScanInfo>,
    ) -> Self {
        ProcessTarget::Instance {
            integrity: ScanIntegrity {
                manifest_path,
                manifest_exist,
                instance_path,
                instance_exist,
            },
            info: scan_info,
        }
    }

    // TODO for Dir
}
