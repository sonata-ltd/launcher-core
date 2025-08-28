use std::{path::PathBuf, sync::Arc};

use getset::Getters;
use serde::Deserialize;
use thiserror::Error;

use crate::{
    data::db,
    instance::{
        paths::InstancePaths,
        websocket::{OperationWsExt, OperationWsMessageLocked},
    },
    utils::{download::Downloadable, maven},
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

#[derive(Debug)]
pub struct SyncResult {
    classpaths: Vec<String>,
    natives_paths: Vec<PathBuf>,
}

pub struct LibsData<'a, 'b> {
    manifest: &'b serde_json::Value,
    paths: Arc<&'a InstancePaths>,
    ws_status: OperationWsMessageLocked<'a>,
    db: &'a db::Database,
    current_os: &'a str,
}

#[derive(Eq, Hash, PartialEq, Debug, Clone, Deserialize, sqlx::FromRow, Getters)]
pub struct LibInfo {
    #[get = "pub"]
    hash: String,
    #[get = "pub"]
    name: String,
    path: String,
    #[get = "pub"]
    url: String,
    native: bool,
}

impl Downloadable for LibInfo {
    fn get_name(&self) -> &String {
        self.name()
    }

    fn get_hash(&self) -> &String {
        self.hash()
    }

    fn get_url(&self) -> &String {
        self.url()
    }
}


const STAGE_TYPE: OperationStage = OperationStage::DownloadLibs;

impl<'a, 'b> LibsData<'a, 'b> {
    pub async fn sync_libs(
        manifest: &'b serde_json::Value,
        paths: &'a InstancePaths,
        ws_status: OperationWsMessageLocked<'a>,
        db: &'a db::Database,
        manifest_type: ManifestType,
    ) -> Result<SyncResult, String> {
        // Sync status through WebSocket
        ws_status
            .clone()
            .start_stage_determinable(STAGE_TYPE, None, 0, 0)
            .await;

        let current_os = match construct_os_name() {
            Ok(name) => name,
            Err(e) => return Err(e.to_string()),
        };

        let paths = Arc::new(paths);
        let libs_data = LibsData {
            manifest,
            paths,
            ws_status: ws_status.clone(),
            db,
            current_os,
        };

        let result = match manifest_type {
            ManifestType::Official => Self::parse_manifest_official(&libs_data).await,
            ManifestType::Prism => Self::parse_manifest_prism(&libs_data).await,
        };

        let sync_data = match result {
            Ok(data) => data,
            Err(e) => return Err(e),
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
        result.natives_paths
    }

    pub fn build_maven_file_path(&self, maven_path: &str) -> String {
        maven::build_file_path(self.paths.libs(), maven_path)
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

    pub fn get_dir_path_and_file_name(&self) -> Option<(PathBuf, PathBuf)> {
        match self.path.rfind("/") {
            Some(pos) => {
                return Some((
                    PathBuf::from(self.path[..pos].to_string()),
                    PathBuf::from(self.path[pos..].to_string()),
                ))
            }
            None => None,
        }
    }
}
