use crate::{instance::download::http, utils::files};

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
        let path = self
            .cache
            .join(format!("{}.json", files::sha256(url.as_bytes())));
        let cached = async_std::fs::read(&path).await.ok();
        let valid = |bytes: &[u8]| {
            expected.is_none_or(|hash| files::sha256(bytes).eq_ignore_ascii_case(hash))
        };

        let fresh = async_std::fs::metadata(&path)
            .await
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|time| SystemTime::now().duration_since(time).ok())
            .is_some_and(|age| age < Duration::from_secs(3600));

        if let Some(bytes) = &cached {
            if valid(bytes) && (expected.is_some() || fresh) {
                return Ok(bytes.clone());
            }
        }

        match http::get_bytes(url, 32 * 1024 * 1024).await {
            Ok(bytes) => {
                if !valid(&bytes) {
                    return Err(ModloaderError::Invalid(format!(
                        "metadata hash mismatch: {url}"
                    )));
                }

                let body = bytes.clone();
                async_std::task::spawn_blocking(move || files::atomic_write(&path, &body)).await?;
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
