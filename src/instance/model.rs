use std::{ffi::OsString, fmt};

use serde::{Deserialize, Serialize};

use crate::{instance::scan::ScanMessage, modloader::model::LoaderKind};

#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[serde(transparent)]
#[sqlx(transparent)]
pub struct InstanceId(i64);

impl InstanceId {
    pub fn get(self) -> i64 {
        self.0
    }
}

impl From<i64> for InstanceId {
    fn from(value: i64) -> Self {
        Self(value)
    }
}

impl fmt::Display for InstanceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

#[derive(Debug, Clone)]
pub struct InstanceRecord {
    pub id: InstanceId,
    pub name: String,
    pub dir: String,
    pub version: String,
    pub loader: LoaderKind,
    pub loader_version: Option<String>,
    pub manifest_url: String,
    pub meta_provider: String,
}

impl InstanceRecord {
    pub fn to_scan_msg(&self) -> ScanMessage {
        super::scan::project(self, None)
    }
}

pub struct NewInstance {
    pub name: String,
    pub dir: String,
    pub version: String,
    pub loader: LoaderKind,
    pub loader_version: Option<String>,
    pub manifest_url: String,
    pub meta_provider: String,
}

pub struct ModId(pub i64);

#[derive(Debug)]
pub struct InstanceMod {
    pub instance_id: InstanceId,
    pub name: OsString,
}

pub struct ModMetadata {
    pub name: Option<String>,
    pub version: Option<String>,
    pub authors: Vec<String>,
    pub icon_url: Option<String>,
}
