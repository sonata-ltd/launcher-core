use std::path::{Path, PathBuf};
use thiserror::Error;

use async_std::fs;

use crate::utils::download::download_in_json;

#[derive(Error, Debug)]
pub enum ManifestError {
    #[error("manifest url has no file name: {0}")]
    UrlWithoutFileName(String),

    #[error("failed to write manifest to {path}: {source}")]
    WriteFailed {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("failed to serialize manifest: {0}")]
    SerializeFailed(#[from] serde_json::Error),

    #[error("failed to parse manifest: {0}")]
    ManifestMalformed(String),

    #[error("download failed: {0}")]
    DownloadFailed(String),
}

pub async fn fetch_manifest(url: &str) -> Result<serde_json::Value, ManifestError> {
    download_in_json(url)
        .await
        .map_err(|e| ManifestError::DownloadFailed(e.to_string()))
}

pub async fn fetch_and_cache_manifest<P: AsRef<Path>>(
    url: &str,
    save_path: P,
) -> Result<(serde_json::Value, PathBuf), ManifestError> {
    let data = fetch_manifest(url).await?;

    let file_name = file_name_from_url(url)
        .ok_or_else(|| ManifestError::UrlWithoutFileName(url.to_string()))?;

    let save_dir = save_path.as_ref();
    let full_path = save_dir.join(file_name);

    fs::create_dir_all(save_dir)
        .await
        .map_err(|source| ManifestError::WriteFailed {
            path: save_dir.to_path_buf(),
            source,
        })?;

    let body = serde_json::to_vec_pretty(&data)?;

    fs::write(&full_path, body)
        .await
        .map_err(|source| ManifestError::WriteFailed {
            path: full_path.clone(),
            source,
        })?;

    Ok((data, full_path))
}

pub async fn load_or_fetch<P: AsRef<Path>>(
    url: &str,
    save_dir: P,
) -> Result<serde_json::Value, ManifestError> {
    let save_dir = save_dir.as_ref();

    if let Some(file_name) = file_name_from_url(url) {
        let cached = save_dir.join(file_name);

        match fs::read(&cached).await {
            Ok(body) => match serde_json::from_slice(&body) {
                Ok(value) => return Ok(value),
                Err(e) => tracing::warn!(
                    path = %cached.display(),
                    error = %e,
                    "cached manifest is malformed, refetching"
                ),
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => tracing::warn!(
                path = %cached.display(),
                error = %e,
                "failed to read cached manifest, refetching"
            ),
        }
    }

    Ok(fetch_and_cache_manifest(url, save_dir).await?.0)
}

pub fn asset_index_id(version_manifest: &serde_json::Value) -> Result<&str, ManifestError> {
    version_manifest
        .get("assetIndex")
        .and_then(|v| v.get("id"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| ManifestError::ManifestMalformed("missing assetIndex.id".to_string()))
}

fn file_name_from_url(url: &str) -> Option<&str> {
    let path = url.split(['?', '#']).next()?;
    path.rsplit('/').find(|seg| !seg.is_empty())
}

pub async fn get_assets_manifest<'a, P: AsRef<Path>>(
    version_manifest: &'a serde_json::Value,
    save_dir: P,
) -> Result<(serde_json::Value, &'a str), ManifestError> {
    let asset_index = version_manifest
        .get("assetIndex")
        .and_then(|v| v.as_object())
        .ok_or(ManifestError::ManifestMalformed(
            "missing assetIndex".to_string(),
        ))?;

    let asset_url =
        asset_index
            .get("url")
            .and_then(|v| v.as_str())
            .ok_or(ManifestError::ManifestMalformed(
                "missing assetIndex.url".to_string(),
            ))?;

    let asset_id = asset_index_id(version_manifest)?;

    let manifest = fetch_and_cache_manifest(asset_url, save_dir).await?;

    Ok((manifest.0, asset_id))
}
