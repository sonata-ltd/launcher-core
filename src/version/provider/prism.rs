use std::collections::HashSet;

use crate::utils::{files::digest::Sha256Digest, url};

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

        if index.format_version != 1 || index.uid != "net.minecraft" {
            return Err(VersionError::InvalidIndex(format!(
                "expected Prism v1 net.minecraft, got format={} uid={}",
                index.format_version, index.uid
            )));
        }

        let mut ids = HashSet::new();
        let mut versions = Vec::with_capacity(index.versions.len());
        for version in index.versions {
            let id = version.version;
            if id.trim().is_empty() || !ids.insert(id.clone()) {
                return Err(VersionError::InvalidIndex(format!(
                    "empty or duplicate version id: {id:?}"
                )));
            }
            url::validate_segment(&id)
                .map_err(|error| VersionError::InvalidIndex(error.to_string()))?;
            let url = url::append_segments(self.base_url(), &[&index.uid, &format!("{id}.json")])
                .map_err(|error| VersionError::InvalidIndex(error.to_string()))?;
            let digest = Sha256Digest::try_from(version.sha256.as_str()).map_err(|error| {
                VersionError::InvalidIndex(format!("invalid hash for version {id:?}: {error}"))
            })?;
            versions.push(VersionSummary {
                id,
                url,
                kind: version.kind,
                latest_stable: version.recommended,
                released_at: Some(version.release_time),
                provider: self.id(),
                sha256: Some(digest.into()),
            });
        }

        Ok(versions)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn index(version: &str, hash: &str) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "formatVersion": 1,
            "name": "Minecraft",
            "uid": "net.minecraft",
            "versions": [{
                "recommended": false,
                "releaseTime": "2026-01-01T00:00:00Z",
                "sha256": hash,
                "type": "release",
                "version": version,
            }],
        }))
        .unwrap()
    }

    #[test]
    fn validates_index_digest_and_builds_one_url_path_segment() {
        let source = PrismSource;
        let hash = "AB".repeat(32);
        for id in ["..", "a/b", "a\\b"] {
            assert!(matches!(
                source.parse_index(&index(id, &hash)),
                Err(VersionError::InvalidIndex(_))
            ));
        }
        assert!(matches!(
            source.parse_index(&index("1.0", &"Q".repeat(64))),
            Err(VersionError::InvalidIndex(_))
        ));
        let rows = source.parse_index(&index("a ?#", &hash)).unwrap();
        assert_eq!(
            rows[0].url,
            "https://meta.prismlauncher.org/v1/net.minecraft/a%20%3F%23.json"
        );
        assert_eq!(
            rows[0].sha256.as_deref(),
            Some(hash.to_ascii_lowercase().as_str())
        );
    }
}
