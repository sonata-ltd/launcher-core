use super::transfer::error::{DownloadError, Result};
use futures::AsyncReadExt;
use std::{io, time::Duration};

#[derive(Debug, thiserror::Error)]
pub(crate) enum HttpError {
    #[error("invalid URL: {0}")]
    InvalidUrl(#[from] http_types::url::ParseError),
    #[error("HTTP request failed: {0}")]
    Request(surf::Error),
    #[error("HTTP {0}")]
    Status(surf::StatusCode),
    #[error("response body read failed: {0}")]
    Body(#[source] io::Error),
    #[error("response exceeds {0} bytes")]
    SizeLimit(usize),
    #[error("HTTP request timed out")]
    Timeout,
}

pub(crate) fn request(url: &str) -> std::result::Result<surf::RequestBuilder, HttpError> {
    Ok(surf::get(surf::Url::parse(url)?))
}

pub(crate) async fn send(
    request: surf::RequestBuilder,
) -> std::result::Result<surf::Response, HttpError> {
    request.send().await.map_err(HttpError::Request)
}

/// Apply one deadline to the request and its bounded response body.
pub(crate) async fn fetch(
    request: surf::RequestBuilder,
    limit: usize,
    deadline: Duration,
) -> std::result::Result<Vec<u8>, HttpError> {
    async_std::future::timeout(deadline, async {
        read_response(send(request).await?, limit).await
    })
    .await
    .map_err(|_| HttpError::Timeout)?
}

pub(crate) async fn read_response(
    mut response: surf::Response,
    limit: usize,
) -> std::result::Result<Vec<u8>, HttpError> {
    if !response.status().is_success() {
        return Err(HttpError::Status(response.status()));
    }

    let mut body = Vec::new();
    response
        .take_body()
        .into_reader()
        .take((limit as u64).saturating_add(1))
        .read_to_end(&mut body)
        .await
        .map_err(HttpError::Body)?;

    if body.len() > limit {
        return Err(HttpError::SizeLimit(limit));
    }

    Ok(body)
}

pub async fn get_bytes(url: &str, limit: usize) -> Result<Vec<u8>> {
    let result = match request(url) {
        Ok(request) => fetch(request, limit, Duration::from_secs(30)).await,
        Err(error) => Err(error),
    };
    result.map_err(|error| match error {
        HttpError::SizeLimit(limit) => DownloadError::SizeLimit {
            limit: limit as u64,
        },
        HttpError::Timeout => DownloadError::Timeout("metadata response"),
        HttpError::Body(source) => DownloadError::Io(source),
        other => DownloadError::Http(format!("{other}: {url}")),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use httpmock::{Method::GET, MockServer};

    #[async_std::test]
    async fn invalid_manifest_url_is_a_typed_error() {
        assert!(matches!(
            crate::utils::download::download_in_json("not-a-url").await,
            Err(crate::utils::download::error::DownloadError::InvalidUrl { .. })
        ));
    }

    #[async_std::test]
    async fn error_status_takes_precedence_over_body_limit() {
        let mut response = http_types::Response::new(surf::StatusCode::NotFound);
        response.set_body("this error response is not parsed");
        assert!(matches!(
            read_response(response.into(), 0).await,
            Err(HttpError::Status(surf::StatusCode::NotFound))
        ));
    }

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
