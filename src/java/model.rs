use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize)]
pub struct JavaRuntime {
    pub id: i64,
    pub version: String,
    pub exec_path: String,
    pub home_path: String,
    pub vendor: Option<String>,
}

/// New external java runtime structure
#[derive(Debug, Deserialize)]
pub struct NewJavaRuntime {
    pub version: String,
    pub exec_path: PathBuf,
    pub home_path: PathBuf,
    pub vendor: Option<String>,
}
