use std::{collections::HashSet, fmt, path::PathBuf};

use serde::{Deserialize, Serialize};

use crate::modloader::artifacts::Wanted;

use super::error::{ModloaderError, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LoaderKind {
    Vanilla,
    Fabric,
    Forge,
    #[serde(rename = "neoforge")]
    NeoForge,
    Quilt,
    Cleanroom,
}

impl LoaderKind {
    pub const ALL: [Self; 6] = [
        Self::Vanilla,
        Self::Fabric,
        Self::Forge,
        Self::NeoForge,
        Self::Quilt,
        Self::Cleanroom,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Vanilla => "vanilla",
            Self::Fabric => "fabric",
            Self::Forge => "forge",
            Self::NeoForge => "neoforge",
            Self::Quilt => "quilt",
            Self::Cleanroom => "cleanroom",
        }
    }
}

impl fmt::Display for LoaderKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Vanilla => "Vanilla",
            Self::Fabric => "Fabric",
            Self::Forge => "Forge",
            Self::NeoForge => "NeoForge",
            Self::Quilt => "Quilt",
            Self::Cleanroom => "Cleanroom",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoaderSpec {
    pub kind: LoaderKind,
    pub version: Option<String>,
}

impl LoaderSpec {
    pub fn validate(&self, version: &str) -> Result<()> {
        match (self.kind, self.version.as_deref()) {
            (LoaderKind::Vanilla, None) => {}
            (LoaderKind::Vanilla, Some(_)) => {
                return Err(ModloaderError::Invalid(
                    "vanilla has no loader version".into(),
                ));
            }
            (_, Some(v)) if !v.trim().is_empty() => {}
            _ => return Err(ModloaderError::Invalid("choose a loader version".into())),
        }

        if self.kind == LoaderKind::Cleanroom && version != "1.12.2" {
            return Err(ModloaderError::Invalid(
                "Cleanroom requires Minecraft 1.12.2".into(),
            ));
        }

        Ok(())
    }

    pub const fn vanilla() -> Self {
        Self {
            kind: LoaderKind::Vanilla,
            version: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JavaPolicy {
    pub majors: Vec<u32>,
    pub allow_newer: bool,
}

impl JavaPolicy {
    pub fn accepts(&self, major: u32) -> bool {
        self.majors.contains(&major)
            || (self.allow_newer && self.majors.iter().max().is_some_and(|max| major > *max))
    }

    pub fn intersect(&self, other: &Self) -> Result<Self> {
        let mut seen = HashSet::new();
        let majors: Vec<u32> = self
            .majors
            .iter()
            .chain(&other.majors)
            .copied()
            .filter(|m| self.accepts(*m) && other.accepts(*m) && seen.insert(*m))
            .collect();

        if majors.is_empty() {
            return Err(ModloaderError::Invalid(
                "incompatible Java requirements".into(),
            ));
        }

        Ok(Self {
            majors,
            allow_newer: self.allow_newer && other.allow_newer,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetadataPin {
    pub url: String,
    pub sha256: String,
}

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
