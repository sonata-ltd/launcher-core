use serde::{Deserialize, Serialize};

use crate::data::registry::operation::message::status::ProgressUnit;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TargetStatus {
    File(FileStatus),
    Dir(DirStatus),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DirStatus {
    Created,
    FailedToCreate,
}
