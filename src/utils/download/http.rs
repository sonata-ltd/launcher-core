use super::{
    error::DownloadError as TransportError,
    transfer::error::{DownloadError, Result},
};
use futures::AsyncReadExt;
use std::{io, time::Duration};

#[derive(Debug, thiserror::Error)]
pub(crate) enum HttpError {
    #[error("invalid URL: {0}")]
    InvalidUrl(#[from] http_types::url::ParseError),
    #[error("unsupported URL scheme")]
    UnsupportedScheme,
    #[error("credentials embedded in download URLs are unsupported")]
    UrlCredentials,
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

impl HttpError {
    pub(crate) fn is_transient(&self) -> bool {
        match self {
            Self::Request(_) | Self::Timeout => true,
            Self::Status(status) => super::error::transient_status(*status),
            Self::Body(error) => super::error::transient_io(error),
            _ => false,
        }
    }
}

pub(crate) fn request(url: &str) -> std::result::Result<surf::RequestBuilder, HttpError> {
    request_with_policy(url, false)
}

pub(crate) fn request_with_policy(
    url: &str,
    https_only: bool,
) -> std::result::Result<surf::RequestBuilder, HttpError> {
    let url = surf::Url::parse(url)?;
    validate_url(&url, https_only)?;
    Ok(surf::get(url))
}

pub(crate) fn validate_url(
    url: &surf::Url,
    https_only: bool,
) -> std::result::Result<(), HttpError> {
    if !(url.scheme() == "https" || !https_only && url.scheme() == "http") {
        return Err(HttpError::UnsupportedScheme);
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(HttpError::UrlCredentials);
    }
    Ok(())
}

pub(crate) fn diagnostic_url(input: &str) -> String {
    let Ok(mut url) = surf::Url::parse(input) else {
        return "<invalid URL>".into();
    };
    let _ = url.set_username("");
    let _ = url.set_password(None);
    url.set_query(None);
    url.set_fragment(None);
    url.into()
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
        other => {
            let url = diagnostic_url(url);
            let error = match other {
                HttpError::InvalidUrl(source) => TransportError::InvalidUrl { url, source },
                HttpError::UnsupportedScheme => TransportError::UnsupportedScheme {
                    url,
                    https_only: false,
                },
                HttpError::UrlCredentials => TransportError::UrlCredentials { url },
                HttpError::Status(status) => TransportError::Http { status, url },
                source => TransportError::Request {
                    url,
                    source: source.into(),
                },
            };
            DownloadError::Network(Box::new(error))
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use httpmock::{Method::GET, MockServer};

    #[test]
    fn url_policy_rejects_embedded_credentials_before_client_initialization() {
        let url = surf::Url::parse("https://fixture:placeholder@example.invalid/path").unwrap();
        assert!(matches!(
            validate_url(&url, false),
            Err(HttpError::UrlCredentials)
        ));
        assert_eq!(diagnostic_url(url.as_str()), "https://example.invalid/path");
    }

    #[async_std::test]
    async fn invalid_manifest_url_is_a_typed_error() {
        assert!(matches!(
            crate::utils::download::download_in_json("not-a-url").await,
            Err(crate::utils::download::error::DownloadError::InvalidUrl { .. })
        ));
    }

    #[async_std::test]
    async fn metadata_rejects_unsupported_schemes_without_exposing_url_secrets() {
        let temp = tempfile::tempdir().unwrap();
        let mut url = surf::Url::from_file_path(temp.path().join("absent")).unwrap();
        url.set_query(Some("token=fixture-secret"));
        let error = get_bytes(url.as_str(), 1024).await.unwrap_err();
        assert!(
            matches!(&error, DownloadError::Network(source)
                if matches!(source.as_ref(), super::super::error::DownloadError::UnsupportedScheme { .. })),
            "{error:?}"
        );
        assert!(!error.to_string().contains("fixture-secret"));
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

        let error = get_bytes(&server.url("/missing?token=fixture-secret"), 1024)
            .await
            .unwrap_err();
        assert!(matches!(&error, DownloadError::Network(source)
            if matches!(source.as_ref(), TransportError::Http { status: surf::StatusCode::NotFound, .. })));
        assert!(!error.to_string().contains("fixture-secret"));
        mock.assert_hits(1);
    }
}
