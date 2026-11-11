use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::modloader::model::{JavaPolicy, LoaderSpec, MetadataPin};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstalledArtifact {
    pub path: PathBuf,
    pub sha1: String,
    pub classpath: bool,
    pub native: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstalledProfile {
    pub format_version: u32,
    pub minecraft: String,
    pub loader: LoaderSpec,
    pub manifest: serde_json::Value,
    pub java: JavaPolicy,
    pub libraries_root: PathBuf,
    pub artifacts: Vec<InstalledArtifact>,
    pub metadata: Vec<MetadataPin>,
}
