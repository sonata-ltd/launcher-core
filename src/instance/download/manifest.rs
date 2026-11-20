use std::path::{Path, PathBuf};
use thiserror::Error;

#[cfg(test)]
use async_std::fs;

use crate::utils::{
    download::{download_in_json, metadata_cache},
    files::{directory::Directory, FileError},
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
    let full_path = metadata_cache::entry_path(save_path.as_ref(), url);
    let entry = manifest_cache(save_path.as_ref(), url, true)
        .await
        .map_err(|source| ManifestError::WriteFailed {
            path: full_path.clone(),
            source,
        })?;
    let data = fetch_into(url, entry, &full_path).await?;
    Ok((data, full_path))
}

async fn fetch_into(
    url: &str,
    entry: metadata_cache::Entry,
    full_path: &Path,
) -> Result<serde_json::Value, ManifestError> {
    let data = fetch_manifest(url).await?;
    let body = serde_json::to_vec(&data)?;
    entry
        .publish(body)
        .await
        .map_err(|source| ManifestError::WriteFailed {
            path: full_path.to_owned(),
            source,
        })?;
    Ok(data)
}

pub async fn load_or_fetch<P: AsRef<Path>>(
    url: &str,
    save_dir: P,
) -> Result<serde_json::Value, ManifestError> {
    let save_dir = save_dir.as_ref();

    let cached = metadata_cache::entry_path(save_dir, url);
    let entry = manifest_cache(save_dir, url, true)
        .await
        .map_err(|source| ManifestError::WriteFailed {
            path: cached.clone(),
            source,
        })?;
    match entry.read().await {
        Ok((body, _)) => match serde_json::from_slice(&body) {
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

    fetch_into(url, entry, &cached).await
}

/// The manifest `load_or_fetch` cached for `url`, if there is one. Never
/// touches the network.
pub async fn load_cached<P: AsRef<Path>>(url: &str, save_dir: P) -> Option<serde_json::Value> {
    let entry = manifest_cache(save_dir.as_ref(), url, false).await.ok()?;
    let (body, _) = entry.read().await.ok()?;

    serde_json::from_slice(&body).ok()
}

async fn manifest_cache(
    root: &Path,
    url: &str,
    create: bool,
) -> std::io::Result<metadata_cache::Entry> {
    let root = root.to_owned();
    let directory = async_std::task::spawn_blocking(move || {
        // An explicit metadata root override may be an alias. Resolve it once
        // before either reading the cache or starting a download.
        if create {
            std::fs::create_dir_all(&root)?;
        }
        Directory::open(&root.canonicalize()?)
    })
    .await?;
    Ok(metadata_cache::Entry::new(directory, url))
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

    #[cfg(unix)]
    #[async_std::test]
    async fn an_explicit_metadata_root_alias_keeps_its_cached_manifest() {
        let scope = tempfile::tempdir().unwrap();
        let actual = scope.path().join("meta");
        let configured = scope.path().join("configured");
        std::fs::create_dir(&actual).unwrap();
        std::os::unix::fs::symlink(&actual, &configured).unwrap();
        let url = "https://example.invalid/manifest.json";
        std::fs::write(
            metadata_cache::entry_path(&actual, url),
            br#"{"id":"cached"}"#,
        )
        .unwrap();

        assert_eq!(
            load_or_fetch(url, &configured).await.unwrap(),
            json!({"id": "cached"})
        );
    }

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
