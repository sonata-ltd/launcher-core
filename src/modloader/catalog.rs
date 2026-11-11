use crate::utils::{files, url};

use super::error::{ModloaderError, Result};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct ComponentIndex {
    #[serde(rename = "formatVersion")]
    pub format_version: u32,
    pub uid: String,
    pub versions: Vec<ComponentVersion>,
}

#[derive(Debug, Deserialize)]
pub struct ComponentVersion {
    pub version: String,
    pub sha256: String,
    #[serde(default)]
    pub recommended: bool,
    #[serde(default)]
    pub requires: Vec<ComponentRequirement>,
}

#[derive(Debug, Deserialize)]
pub struct ComponentRequirement {
    pub uid: String,
    pub equals: Option<String>,
}

pub fn compatible_versions(bytes: &[u8], uid: &str, mc: &str) -> Result<Vec<ComponentVersion>> {
    let index: ComponentIndex = serde_json::from_slice(bytes)?;

    if index.format_version != 1 || index.uid != uid {
        return Err(ModloaderError::Unsupported(
            "unexpected Prism component index".into(),
        ));
    }

    Ok(index
        .versions
        .into_iter()
        .filter(|version| {
            version
                .requires
                .iter()
                .any(|req| req.uid == "net.minecraft" && req.equals.as_deref() == Some(mc))
        })
        .collect())
}

#[derive(Debug, Clone)]
pub struct LoaderVersionSummary {
    pub version: String,
    pub recommended: bool,
    pub prerelease: bool,
    pub component_url: Option<String>,
    pub sha256: Option<String>,
}

pub struct ModloaderCatalog {
    pub repository: super::repository::Repository,
}

impl ModloaderCatalog {
    pub fn new(cache: std::path::PathBuf) -> Self {
        Self {
            repository: super::repository::Repository::new(cache),
        }
    }

    pub async fn versions(
        &self,
        kind: super::model::LoaderKind,
        mc: &str,
    ) -> Result<Vec<LoaderVersionSummary>> {
        use super::model::LoaderKind as K;

        match kind {
            K::Vanilla => Ok(Vec::new()),
            K::Fabric | K::Quilt => Ok(super::fabric_quilt::versions(kind, mc)
                .await?
                .into_iter()
                .map(|v| LoaderVersionSummary {
                    version: v.loader.version,
                    recommended: v.loader.stable,
                    prerelease: !v.loader.stable,
                    component_url: None,
                    sha256: None,
                })
                .collect()),
            K::Forge | K::NeoForge => {
                let uid = if kind == K::Forge {
                    "net.minecraftforge"
                } else {
                    "net.neoforged"
                };

                let url = url::append_segments(
                    "https://meta.prismlauncher.org/v1/",
                    &[uid, "index.json"],
                )?;

                let bytes = self.repository.bytes(&url, None).await?;
                let mut seen = std::collections::HashSet::new();

                compatible_versions(&bytes, uid, mc)?
                    .into_iter()
                    .map(|entry| {
                        if !seen.insert(entry.version.clone())
                            || !files::hash_valid(&entry.sha256, 64)
                        {
                            return Err(ModloaderError::Invalid(
                                "invalid/duplicate component version".into(),
                            ));
                        }

                        let url = url::append_segments(
                            "https://meta.prismlauncher.org/v1/",
                            &[uid, &format!("{}.json", entry.version)],
                        )?;

                        Ok(LoaderVersionSummary {
                            prerelease: entry.version.contains("beta")
                                || entry.version.contains("alpha"),
                            version: entry.version,
                            recommended: entry.recommended,
                            component_url: Some(url),
                            sha256: Some(entry.sha256),
                        })
                    })
                    .collect()
            }
            K::Cleanroom => Ok(super::cleanroom::get_releases(&self.repository)
                .await?
                .into_iter()
                .filter(|v| v.minecraft == mc)
                .map(|v| LoaderVersionSummary {
                    version: v.version,
                    recommended: false,
                    prerelease: true,
                    component_url: None,
                    sha256: Some(v.archive_sha256),
                })
                .collect()),
        }
    }
}
