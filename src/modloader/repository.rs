use crate::utils::{
    download::{http, metadata_cache},
    files::{self, directory::Directory},
};

use super::error::{ModloaderError, Result};
use serde_json::Value;
use std::{
    path::PathBuf,
    time::{Duration, SystemTime},
};

#[derive(Clone)]
pub struct Repository {
    pub cache: PathBuf,
    pins: std::collections::HashMap<String, String>,
}

impl Repository {
    pub fn new(cache: PathBuf) -> Self {
        Self {
            cache,
            pins: Default::default(),
        }
    }

    pub fn pinned(&self, pins: &[super::model::MetadataPin]) -> Self {
        Self {
            cache: self.cache.clone(),
            pins: pins
                .iter()
                .map(|pin| (pin.url.clone(), pin.sha256.clone()))
                .collect(),
        }
    }

    pub async fn bytes(&self, url: &str, expected: Option<&str>) -> Result<Vec<u8>> {
        let pinned = self.pins.get(url).map(String::as_str);

        if let (Some(pinned), Some(advertised)) = (pinned, expected) {
            if !pinned.eq_ignore_ascii_case(advertised) {
                return Err(ModloaderError::Invalid(
                    "metadata changed for a pinned loader version".into(),
                ));
            }
        }

        let expected = pinned.or(expected);
        let root = self.cache.clone();
        let directory = async_std::task::spawn_blocking(move || {
            Directory::open_child_of(&root, true)?
                .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::NotFound))
        })
        .await?;
        let entry = metadata_cache::Entry::new(directory, url);
        let (cached, modified) = match entry.read().await {
            Ok((bytes, modified)) => (Some(bytes), modified),
            Err(_) => (None, None),
        };
        let valid = |bytes: &[u8]| {
            expected.is_none_or(|hash| files::sha256(bytes).eq_ignore_ascii_case(hash))
        };

        let fresh = modified
            .and_then(|time| SystemTime::now().duration_since(time).ok())
            .is_some_and(|age| age < Duration::from_secs(3600));

        if let Some(bytes) = &cached {
            if valid(bytes) && (expected.is_some() || fresh) {
                return Ok(bytes.clone());
            }
        }

        match http::get_bytes(url, crate::utils::download::MAX_MANIFEST_BYTES).await {
            Ok(bytes) => {
                if !valid(&bytes) {
                    return Err(ModloaderError::Invalid(format!(
                        "metadata hash mismatch: {}",
                        http::diagnostic_url(url)
                    )));
                }

                entry.publish(bytes.clone()).await?;
                Ok(bytes)
            }
            Err(error) => match cached {
                Some(bytes) if valid(&bytes) => Ok(bytes),
                _ => Err(error.into()),
            },
        }
    }

    pub async fn json(&self, url: &str, expected: Option<&str>) -> Result<Value> {
        Ok(serde_json::from_slice(&self.bytes(url, expected).await?)?)
    }
}
