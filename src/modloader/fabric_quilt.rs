use http_types::Url;
use serde::Deserialize;
use serde_json::Value;

use crate::modloader::{files, model::MetadataPin};

use super::{
    error::{ModloaderError, Result},
    http::get_bytes,
    model::LoaderKind,
    repository::Repository,
};

#[derive(Debug, Clone, Deserialize)]
pub struct MetaLoader {
    pub version: String,
    #[serde(default)]
    pub stable: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MetaLoaderEntry {
    pub loader: MetaLoader,
}

const FABRIC_ENDPOINT: &'static str = "https://meta.fabricmc.net/v2/versions/loader/";
const QUILT_ENDPOINT: &'static str = "https://meta.quiltmc.org/v3/versions/loader/";

fn endpoint(kind: LoaderKind, segments: &[&str]) -> Result<Url> {
    let base = match kind {
        LoaderKind::Fabric => FABRIC_ENDPOINT,
        LoaderKind::Quilt => QUILT_ENDPOINT,
        _ => return Err(ModloaderError::Invalid("expected Fabric or Quilt".into())),
    };

    let mut url = Url::parse(base).map_err(|e| ModloaderError::Invalid(e.to_string()))?;
    {
        let mut path = url
            .path_segments_mut()
            .map_err(|_| ModloaderError::Invalid("invalid Meta base URL".into()))?;
        path.pop_if_empty();

        for segment in segments {
            if segment.is_empty()
                || matches!(*segment, "." | "..")
                || segment.contains('/')
                || segment.contains('\\')
            {
                return Err(ModloaderError::Invalid("invalid Meta path segment".into()));
            }

            path.push(segment);
        }
    }

    Ok(url)
}

pub async fn versions(kind: LoaderKind, mc: &str) -> Result<Vec<MetaLoaderEntry>> {
    let url = endpoint(kind, &[mc])?;
    let bytes = get_bytes(url.as_str(), 4 * 1024 * 1024).await?;

    Ok(serde_json::from_slice(&bytes)?)
}

pub struct LoaderProfile {
    pub manifest: Value,
    pub pin: MetadataPin,
}

pub async fn profile(
    repo: &Repository,
    kind: LoaderKind,
    mc: &str,
    loader: &str,
) -> Result<LoaderProfile> {
    let url = endpoint(kind, &[mc, loader, "profile", "json"])?;

    // The exact profile validates compatibility; a live catalog is unnecessary
    // when repairing a pinned installation offline.
    let manifest_bytes = repo.bytes(url.as_str(), None).await?;
    let manifest_value: Value = serde_json::from_slice(&manifest_bytes)?;

    if manifest_value.get("inheritsFrom").and_then(Value::as_str) != Some(mc) {
        return Err(ModloaderError::Invalid(
            "loader profile targets another Minecraft".into(),
        ));
    }

    if manifest_value
        .get("mainClass")
        .and_then(Value::as_str)
        .is_none()
    {
        return Err(ModloaderError::Invalid(
            "loader profile has no mainClass".into(),
        ));
    }

    let pin = MetadataPin {
        url: url.to_string(),
        sha256: files::sha256(&manifest_bytes),
    };

    Ok(LoaderProfile {
        manifest: manifest_value,
        pin,
    })
}
