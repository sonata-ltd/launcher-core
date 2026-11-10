use http_types::Url;
use serde::Deserialize;
use serde_json::Value;

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

fn endpoint(kind: LoaderKind, segments: &[&str]) -> Result<Url> {
    let base = match kind {
        LoaderKind::Fabric => "https://meta.fabricmc.net/v2/versions/loader/",
        LoaderKind::Quilt => "https://meta.quiltmc.org/v3/versions/loader/",
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

pub async fn profile(repo: &Repository, kind: LoaderKind, mc: &str, loader: &str) -> Result<Value> {
    let url = endpoint(kind, &[mc, loader, "profile", "json"])?;
    // The exact profile validates compatibility; a live catalog is unnecessary
    // when repairing a pinned installation offline.
    let value = repo.json(url.as_str(), None).await?;

    if value.get("inheritsFrom").and_then(Value::as_str) != Some(mc) {
        return Err(ModloaderError::Invalid(
            "loader profile targets another Minecraft".into(),
        ));
    }

    if value.get("mainClass").and_then(Value::as_str).is_none() {
        return Err(ModloaderError::Invalid(
            "loader profile has no mainClass".into(),
        ));
    }

    Ok(value)
}
