use std::path::PathBuf;

use serde::Deserialize;
use thiserror::Error;

use crate::{
    instance::{
        paths::InstancePaths,
        websocket::{OperationWsExt, OperationWsMessageLocked},
    },
    websocket::messages::operation::stage::{OperationStage, StageStatus},
};

mod download;
mod parse;
mod register;

#[derive(Clone, Copy, PartialEq)]
pub enum ManifestType {
    Official,
    Prism,
}

#[derive(Error, Debug)]
pub enum LibsSyncError {
    #[error("OS is not supported")]
    OsNotAvailable,

    #[error("CPU architecture is not supported")]
    ArchNotAvailable,
}

pub struct SyncResult {
    classpaths: Vec<String>,
    natives: Vec<PathBuf>
}

pub struct LibsData<'a, 'b> {
    manifest: &'b serde_json::Value,
    paths: &'a InstancePaths,
    ws_status: OperationWsMessageLocked<'a>,
    current_os: &'a str,
}

#[derive(Eq, Hash, PartialEq, Debug, Clone, Deserialize)]
pub struct LibInfo {
    hash: String,
    name: String,
    path: String,
    url: String,
    native: bool,
    save_path: Option<PathBuf>
}

const STAGE_TYPE: OperationStage = OperationStage::DownloadLibs;

impl<'a, 'b> LibsData<'a, 'b> {
    pub async fn sync_libs(
        manifest: &'b serde_json::Value,
        paths: &'a InstancePaths,
        ws_status: OperationWsMessageLocked<'a>,
        manifest_type: ManifestType,
    ) -> Result<SyncResult, String> {
        // Sync status through WebSocket
        ws_status
            .clone()
            .start_stage_determinable(STAGE_TYPE, None, 0, 0)
            .await;

        let current_os = match construct_os_name() {
            Ok(name) => name,
            Err(e) => return Err(e.to_string())
        };

        let libs_data = LibsData {
            manifest,
            paths,
            ws_status: ws_status.clone(),
            current_os
        };

        let result = match manifest_type {
            ManifestType::Official => {
                Self::parse_manifest_official(libs_data).await
            },
            ManifestType::Prism => {
                Self::parse_manifest_prism(libs_data).await
            }
        };

        let sync_data = match result {
            Ok(data) => {
                data
            },
            Err(e) => return Err(e)
        };

        ws_status
            .complete_stage(StageStatus::Completed, STAGE_TYPE, 0.0, None)
            .await;

        Ok(sync_data)
    }

    pub fn get_classpaths_mut(result: &mut SyncResult) -> &mut Vec<String> {
        &mut result.classpaths
    }

    pub fn take_natives_paths(result: SyncResult) -> Vec<PathBuf> {
        result.natives
    }
}

fn construct_os_name() -> Result<&'static str, LibsSyncError> {
    // Linux
    #[cfg(all(target_os = "linux", target_arch = "x86"))]
    return Ok("linux");

    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    return Ok("linux");

    #[cfg(all(target_os = "linux", target_arch = "arm"))]
    return Ok("linux-arm32");

    #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
    return Ok("linux-arm64");

    // macOS
    #[cfg(all(target_os = "macos", target_arch = "x86"))]
    return Ok("osx");

    #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
    return Ok("osx");

    #[cfg(all(target_os = "macos", target_arch = "arm"))]
    return Ok("osx");

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    return Ok("osx-arm64");

    // Windows
    #[cfg(all(target_os = "windows", target_arch = "x86"))]
    return Ok("windows");

    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    return Ok("windows");

    #[cfg(all(target_os = "windows", target_arch = "arm"))]
    return Ok("windows-arm32");

    #[cfg(all(target_os = "windows", target_arch = "aarch64"))]
    return Ok("windows-arm64");

    // If OS/arch combination is not supported
    #[allow(unreachable_code)]
    Err(LibsSyncError::OsNotAvailable)
}

impl LibInfo {
    pub fn is_native(&self) -> bool {
        if self.native == true {
            true
        } else {
            false
        }
    }
}
