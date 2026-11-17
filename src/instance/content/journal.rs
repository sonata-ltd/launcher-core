//! Persisted publication records. Legacy journals stored the files array directly.
use serde::{Deserialize, Serialize};

use super::error::{ContentError, Result};

const VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct PublishedFile {
    pub name: String,
    pub sha256: String,
    pub object_id: i64,
    #[serde(serialize_with = "serialize_provider")]
    pub provider: super::model::Provider,
    pub project_id: String,
    pub release_id: String,
    pub placement: Placement,
    #[serde(default)]
    pub dependencies: Option<Vec<super::model::Dependency>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[serde(rename_all = "lowercase")]
#[sqlx(rename_all = "lowercase")]
pub(super) enum Placement {
    External,
    Copy,
    Hardlink,
    Reflink,
}

fn serialize_provider<S: serde::Serializer>(
    provider: &super::model::Provider,
    serializer: S,
) -> std::result::Result<S::Ok, S::Error> {
    serializer.serialize_str(provider.slug())
}

#[derive(Serialize, Deserialize)]
struct Journal<T> {
    version: u32,
    files: T,
}

pub(super) fn encode(files: &[PublishedFile]) -> Result<String> {
    Ok(serde_json::to_string(&Journal {
        version: VERSION,
        files,
    })?)
}

pub(super) fn decode(body: &str) -> Result<Vec<PublishedFile>> {
    if body.trim_start().starts_with('[') {
        return Ok(serde_json::from_str(body)?);
    }

    // Decode the envelope before inspecting the version-specific payload.
    let envelope: Journal<Box<serde_json::value::RawValue>> = serde_json::from_str(body)?;
    if envelope.version != VERSION {
        return Err(ContentError::UnsupportedJournalVersion(envelope.version));
    }
    Ok(serde_json::from_str(envelope.files.get())?)
}
