use super::error::{DownloadError, Result};
use futures::AsyncReadExt;
use std::time::Duration;

pub async fn get_bytes(url: &str, limit: usize) -> Result<Vec<u8>> {
    let fetch = async {
        let mut response = surf::get(url)
            .await
            .map_err(|e| DownloadError::Http(e.to_string()))?;

        if !response.status().is_success() {
            return Err(DownloadError::Http(format!("{}: {url}", response.status())));
        }

        let mut body = Vec::new();
        response
            .take_body()
            .into_reader()
            .take(limit as u64 + 1)
            .read_to_end(&mut body)
            .await?;

        if body.len() > limit {
            return Err(DownloadError::SizeLimit {
                limit: limit as u64,
            });
        }

        Ok(body)
    };

    async_std::future::timeout(Duration::from_secs(30), fetch)
        .await
        .map_err(|_| DownloadError::Timeout("metadata response"))?
}

#[cfg(test)]
mod tests {
    use super::*;
    use httpmock::{Method::GET, MockServer};

    #[async_std::test]
    async fn returns_body_at_limit_and_rejects_larger_response() {
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(GET).path("/metadata");
            then.status(200).body("1234");
        });
        let url = server.url("/metadata");

        assert_eq!(get_bytes(&url, 4).await.unwrap(), b"1234");
        assert!(matches!(
            get_bytes(&url, 3).await,
            Err(DownloadError::SizeLimit { limit: 3 })
        ));
        mock.assert_hits(2);
    }

    #[async_std::test]
    async fn preserves_http_failure_as_download_error() {
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(GET).path("/missing");
            then.status(404);
        });

        assert!(matches!(
            get_bytes(&server.url("/missing"), 1024).await,
            Err(DownloadError::Http(message)) if message.contains("404")
        ));
        mock.assert_hits(1);
    }
}
