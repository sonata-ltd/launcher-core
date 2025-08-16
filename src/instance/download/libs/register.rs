use std::collections::HashSet;

use async_std::{fs::File, io::WriteExt};
use serde_json::json;
use thiserror::Error;

use super::*;

#[derive(Error, Debug)]
pub enum RegisterError {
    #[error("Failed to register: {0}")]
    FailedToRegister(String)
}

impl<'a, 'b> LibsData<'a, 'b> {
    pub async fn register_libs(
        downloaded_libs: &HashSet<LibInfo>,
        mut metacache: serde_json::Value,
        paths: &InstancePaths,
    ) -> Result<(), RegisterError> {
        if let Some(libs) = metacache["libraries"].as_array_mut() {
            for item in downloaded_libs.iter() {
                libs.push(json!({
                    "hash": item.hash,
                    "name": item.name,
                    "path": item.path,
                }));
            }
        }

        let mut metacache_file = File::create(paths.metacache_file()).await.unwrap();
        match metacache_file
            .write_all(serde_json::to_string_pretty(&metacache).unwrap().as_bytes())
            .await {
                Ok(_) => Ok(()),
                Err(e) => Err(RegisterError::FailedToRegister(e.to_string()))
            }
    }
}
