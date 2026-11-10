use super::error::{ModloaderError, Result};
use futures::AsyncReadExt;
use std::time::Duration;

pub async fn get_bytes(url: &str, limit: usize) -> Result<Vec<u8>> {
    let fetch = async {
        let mut responce = surf::get(url)
            .await
            .map_err(|e| ModloaderError::Http(e.to_string()))?;

        if !responce.status().is_success() {
            return Err(ModloaderError::Http(format!(
                "{}: {url}",
                responce.status()
            )));
        }

        let mut body = Vec::new();
        responce
            .take_body()
            .into_reader()
            .take(limit as u64 + 1)
            .read_to_end(&mut body)
            .await?;

        if body.len() > limit {
            return Err(ModloaderError::Http(format!(
                "response exceeds {limit} bytes"
            )));
        }

        Ok(body)
    };

    async_std::future::timeout(Duration::from_secs(30), fetch)
        .await
        .map_err(|_| ModloaderError::Http("metadata request timed out".into()))?
}
