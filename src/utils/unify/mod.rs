use serde::Serialize;
use surf::StatusCode;

use crate::utils::download::download_in_json;

#[derive(Serialize)]
pub struct UnifiedVersion<'a> {
    id: &'a str,
    url: String,
}

pub struct UnifiedVersionsData {
    manifest: serde_json::Value,
    manifest_type: MetaProviders,
}

#[allow(dead_code)]
pub enum MetaProviders {
    Mojang,
    Prism,
}

impl<'a> UnifiedVersionsData {
    pub async fn new(manifest_type: MetaProviders) -> Result<Self, StatusCode> {
        let manifest_url = match manifest_type {
            MetaProviders::Mojang => "https://piston-meta.mojang.com/mc/game/version_manifest_v2.json",
            MetaProviders::Prism => "https://meta.prismlauncher.org/v1/net.minecraft/index.json",
        };

        let value = match download_in_json(manifest_url).await {
            Ok(val) => val,
            Err(e) => return Err(e.status()),
        };

        Ok(UnifiedVersionsData {
            manifest: value,
            manifest_type,
        })
    }

    pub fn build(self) -> Result<String, String> {
        let unified_versions_result = match self.manifest_type {
            MetaProviders::Mojang => Err("Not implemented".to_string()),
            MetaProviders::Prism => Self::extract_prism(&self.manifest),
        };

        match unified_versions_result {
            Ok(data) => match serde_json::to_string_pretty(&data) {
                Ok(data) => Ok(data),
                Err(e) => Err(e.to_string()),
            },
            Err(e) => Err(e),
        }
    }

    fn extract_prism(manifest: &'a serde_json::Value) -> Result<Vec<UnifiedVersion<'a>>, String> {
        let mut unified_versions: Vec<UnifiedVersion> = Vec::new();

        // Get UID to construct url
        if let Some(uid) = manifest.get("uid").and_then(|v| v.as_str()) {
            let base_path = format!("https://meta.prismlauncher.org/v1/{}", uid);

            if let Some(versions) = manifest.get("versions").and_then(|v| v.as_array()) {
                for version in versions {
                    if let Some(id) = version.get("version").and_then(|v| v.as_str()) {
                        let url = format!("{}/{}.json", base_path, id);

                        unified_versions.push(UnifiedVersion { id, url });
                    }
                }

                return Ok(unified_versions);
            }
        }

        Err("Failed to parse".to_string())
    }
}
