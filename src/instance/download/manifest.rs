use std::path::{Path, PathBuf};
use thiserror::Error;

use async_std::fs;

use crate::utils::{
    download::{download_in_json, metadata_cache},
    files::FileError,
};

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
    DownloadFailed(#[source] crate::utils::download::error::DownloadError),

    #[error("invalid asset index JSON: {0}")]
    InvalidAssetIndexJson(#[source] serde_json::Error),

    #[error("asset index {id:?}: {details}")]
    AssetIndexIntegrity { id: String, details: String },

    #[error("asset index filesystem error: {0}")]
    AssetIndexFile(#[from] FileError),
}

pub async fn fetch_manifest(url: &str) -> Result<serde_json::Value, ManifestError> {
    download_in_json(url)
        .await
        .map_err(ManifestError::DownloadFailed)
}

pub async fn fetch_and_cache_manifest<P: AsRef<Path>>(
    url: &str,
    save_path: P,
) -> Result<(serde_json::Value, PathBuf), ManifestError> {
    let data = fetch_manifest(url).await?;

    let full_path = metadata_cache::entry_path(save_path.as_ref(), url);
    let body = serde_json::to_vec(&data)?;
    metadata_cache::publish(full_path.clone(), body)
        .await
        .map_err(|error| ManifestError::WriteFailed {
            path: full_path.clone(),
            source: match error {
                FileError::Io(source) => source,
                other => std::io::Error::new(std::io::ErrorKind::InvalidInput, other),
            },
        })?;

    Ok((data, full_path))
}

pub async fn load_or_fetch<P: AsRef<Path>>(
    url: &str,
    save_dir: P,
) -> Result<serde_json::Value, ManifestError> {
    let save_dir = save_dir.as_ref();

    let cached = metadata_cache::entry_path(save_dir, url);
    match fs::read(&cached).await {
        Ok(body) => match serde_json::from_slice(&body) {
            Ok(value) => return Ok(value),
            Err(error) => tracing::warn!(
                path = %cached.display(),
                %error,
                "cached manifest is malformed, refetching"
            ),
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => tracing::warn!(
            path = %cached.display(),
            %error,
            "failed to read cached manifest, refetching"
        ),
    }

    Ok(fetch_and_cache_manifest(url, save_dir).await?.0)
}

/// The manifest `load_or_fetch` cached for `url`, if there is one. Never
/// touches the network.
pub async fn load_cached<P: AsRef<Path>>(url: &str, save_dir: P) -> Option<serde_json::Value> {
    let cached = metadata_cache::entry_path(save_dir.as_ref(), url);
    let body = fs::read(&cached).await.ok()?;

    serde_json::from_slice(&body).ok()
}

pub fn asset_index_id(version_manifest: &serde_json::Value) -> Result<&str, ManifestError> {
    let id = version_manifest
        .get("assetIndex")
        .and_then(|v| v.get("id"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| ManifestError::ManifestMalformed("missing assetIndex.id".to_string()))?;
    crate::utils::files::portable_name::validate_file_name(id)
        .map_err(|_| ManifestError::ManifestMalformed("unsafe assetIndex.id".into()))?;
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[async_std::test]
    async fn manifests_with_the_same_basename_do_not_share_cached_contents() {
        let directory = tempfile::tempdir().unwrap();
        let first = "https://one.example.invalid/loader/profile.json";
        let second = "https://two.example.invalid/loader/profile.json";
        let other_query = "https://one.example.invalid/loader/profile.json?variant=other";
        let other_path = "https://one.example.invalid/other/profile.json";
        fs::write(directory.path().join("profile.json"), br#"{"id":"legacy"}"#)
            .await
            .unwrap();
        assert_eq!(load_cached(first, directory.path()).await, None);
        for (url, version) in [
            (first, "first"),
            (second, "second"),
            (other_query, "query"),
            (other_path, "path"),
        ] {
            let path = metadata_cache::entry_path(directory.path(), url);
            metadata_cache::publish(path, serde_json::to_vec(&json!({"id": version})).unwrap())
                .await
                .unwrap();
        }
        assert_eq!(
            load_or_fetch(first, directory.path()).await.unwrap(),
            json!({"id":"first"})
        );
        assert_eq!(
            load_cached(second, directory.path()).await,
            Some(json!({"id":"second"}))
        );
        assert_eq!(
            load_cached(other_query, directory.path()).await,
            Some(json!({"id":"query"}))
        );
        assert_eq!(
            load_cached(other_path, directory.path()).await,
            Some(json!({"id":"path"}))
        );
    }
}
