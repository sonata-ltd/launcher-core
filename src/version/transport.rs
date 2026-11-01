use async_trait::async_trait;
use futures::AsyncReadExt;

use super::error::{Result, VersionError};
use super::provider::MetaProvider;

pub(super) const MAX_INDEX_BYTES: usize = 8 * 1024 * 1024;

#[async_trait]
pub(super) trait IndexLoader: Send + Sync {
    async fn load(&self, provider: MetaProvider) -> Result<Vec<u8>>;
}

pub(super) struct HttpIndexLoader;

#[async_trait]
impl IndexLoader for HttpIndexLoader {
    async fn load(&self, provider: MetaProvider) -> Result<Vec<u8>> {
        let url = provider.source()?.index_url();
        load_url(&url).await
    }
}

// Separate helper permits local HTTP tests; GUI never supplies an arbitrary URL.
pub(super) async fn load_url(url: &str) -> Result<Vec<u8>> {
    let operation = async {
        let response = surf::get(url)
            .await
            .map_err(|error| VersionError::IndexUnavailable(error.to_string()))?;
        read_response(response).await
    };

    async_std::future::timeout(crate::utils::download::ONESHOT_TIMEOUT, operation)
        .await
        .map_err(|_| VersionError::IndexUnavailable("version index request timed out".into()))?
}

// Test response validation without requiring a local listening socket.
pub(super) async fn read_response(mut response: surf::Response) -> Result<Vec<u8>> {
    if !response.status().is_success() {
        return Err(VersionError::HttpStatus(u16::from(response.status())));
    }
    let mut bytes = Vec::new();
    response
        .take_body()
        .into_reader()
        .take((MAX_INDEX_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .await
        .map_err(|error| VersionError::IndexUnavailable(error.to_string()))?;
    if bytes.len() > MAX_INDEX_BYTES {
        return Err(VersionError::IndexTooLarge(MAX_INDEX_BYTES));
    }
    Ok(bytes)
}
