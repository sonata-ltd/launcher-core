use crate::version::provider::VersionKind;
use crate::version::provider::{VersionSource, VersionSummary};

use crate::version::error::{Result, VersionError};
use chrono::{DateTime, Utc};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PrismIndex {
    format_version: u32,
    name: String,
    uid: String,
    versions: Vec<PrismVersion>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PrismVersion {
    recommended: bool,
    release_time: DateTime<Utc>,
    #[serde(default)]
    requires: Vec<PrismRequirement>,
    sha256: String,
    #[serde(rename = "type")]
    kind: VersionKind,
    version: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PrismRequirement {
    suggests: String,
    uid: String,
}

pub struct PrismSource;

impl VersionSource for PrismSource {
    fn id(&self) -> super::MetaProvider {
        super::MetaProvider::Prism
    }

    fn base_url(&self) -> &'static str {
        "https://meta.prismlauncher.org/v1"
    }

    fn index_url(&self) -> String {
        format!(
            "{}{}",
            self.base_url().to_owned(),
            "/net.minecraft/index.json".to_string()
        )
    }

    fn parse_index(&self, raw: &[u8]) -> Result<Vec<VersionSummary>> {
        let index = serde_json::from_slice::<PrismIndex>(&raw).map_err(|e| {
            VersionError::MalformedIndex {
                provider: self.id(),
                source: e,
            }
        })?;

        let base_url = self.base_url();
        let mut version_summary = Vec::new();

        for version in index.versions {
            let url = format!("{}/{}/{}.json", base_url, index.uid, version.version);

            version_summary.push(VersionSummary {
                id: version.version,
                url,
                kind: version.kind,
                latest_stable: version.recommended,
                released_at: Some(version.release_time),
                provider: self.id(),
                sha256: Some(version.sha256),
            });
        }

        Ok(version_summary)
    }
}
