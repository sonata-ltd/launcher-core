use crate::utils::download::{
    http::{self, HttpError},
    ONESHOT_TIMEOUT,
};
use async_trait::async_trait;

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
    let request = http::request(url).map_err(map_http_error)?;
    http::fetch(request, MAX_INDEX_BYTES, ONESHOT_TIMEOUT)
        .await
        .map_err(map_http_error)
}

#[cfg(test)]
pub(super) async fn read_response(response: surf::Response) -> Result<Vec<u8>> {
    http::read_response(response, MAX_INDEX_BYTES)
        .await
        .map_err(map_http_error)
}

fn map_http_error(error: HttpError) -> VersionError {
    match error {
        HttpError::Status(status) => VersionError::HttpStatus(status.into()),
        HttpError::SizeLimit(limit) => VersionError::IndexTooLarge(limit),
        other => VersionError::IndexUnavailable(other.to_string()),
    }
}
