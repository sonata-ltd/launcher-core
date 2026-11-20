//! Общий ограниченный JSON transport. Здесь нет знаний о модах и провайдерах.
use super::http::{self, HttpError};
use std::time::Duration;

const MAX_JSON_BYTES: usize = 8 * 1024 * 1024;
use serde::de::DeserializeOwned;

/// Ленивый общий клиент. Config::try_into возвращает ошибку инициализации,
/// в отличие от Client::new, который у curl backend может вызвать panic.
#[derive(Default)]
pub struct JsonClient {
    client: std::sync::OnceLock<
        Result<surf::Client, std::sync::Arc<dyn std::error::Error + Send + Sync>>,
    >,
}

impl JsonClient {
    pub fn get(&self, url: surf::Url) -> Result<surf::RequestBuilder, JsonRequestError> {
        let client = self
            .client
            .get_or_init(|| {
                surf::Client::try_from(surf::Config::new()).map_err(|error| {
                    std::sync::Arc::new(error)
                        as std::sync::Arc<dyn std::error::Error + Send + Sync>
                })
            })
            .as_ref()
            .map_err(|error| JsonRequestError::Transport(error.clone()))?;
        Ok(client.get(url).header(
            "User-Agent",
            "SonataLauncher/0.1 (https://github.com/sonata-ltd/launcher-core)",
        ))
    }
}

pub mod error;
pub use error::JsonRequestError;

/// Весь запрос, включая тело, имеет ограничение времени. redirect middleware
/// не устанавливается: секрет API не переносится на неизвестный origin.
pub async fn get<T: DeserializeOwned>(
    request: surf::RequestBuilder,
) -> Result<T, JsonRequestError> {
    let bytes = http::fetch(request, MAX_JSON_BYTES, Duration::from_secs(25))
        .await
        .map_err(map_http_error)?;
    Ok(serde_json::from_slice(&bytes)?)
}

pub async fn read_response<T: DeserializeOwned>(
    response: surf::Response,
) -> Result<T, JsonRequestError> {
    let bytes = http::read_response(response, MAX_JSON_BYTES)
        .await
        .map_err(map_http_error)?;
    Ok(serde_json::from_slice(&bytes)?)
}

fn map_http_error(error: HttpError) -> JsonRequestError {
    match error {
        HttpError::Status(status) => JsonRequestError::Status(status.into()),
        HttpError::SizeLimit(_) => JsonRequestError::SizeLimit,
        HttpError::Timeout => JsonRequestError::Timeout,
        HttpError::Request(error) => JsonRequestError::Transport(std::sync::Arc::from(Box::<
            dyn std::error::Error + Send + Sync,
        >::from(
            error.into_inner(),
        ))),
        HttpError::Body(error) => JsonRequestError::Body(error),
        HttpError::InvalidUrl(error) => JsonRequestError::InvalidUrl(error),
        HttpError::UnsupportedScheme => JsonRequestError::UnsupportedScheme,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[async_std::test]
    async fn body_at_limit_succeeds_and_one_extra_byte_is_rejected() {
        let mut body = vec![b' '; MAX_JSON_BYTES];
        body[0] = b'0';
        let mut response = http_types::Response::new(http_types::StatusCode::Ok);
        response.set_body(body.clone());
        assert_eq!(
            read_response::<serde_json::Value>(response.into())
                .await
                .unwrap(),
            serde_json::json!(0)
        );
        body.push(b' ');
        let mut response = http_types::Response::new(http_types::StatusCode::Ok);
        response.set_body(body);
        assert!(matches!(
            read_response::<serde_json::Value>(response.into()).await,
            Err(JsonRequestError::SizeLimit)
        ));
    }
    #[async_std::test]
    async fn rate_limit_remains_an_http_error_not_a_json_decode_error() {
        let mut response = http_types::Response::new(http_types::StatusCode::TooManyRequests);
        response.set_body("try later");
        assert!(matches!(
            read_response::<serde_json::Value>(response.into()).await,
            Err(JsonRequestError::Status(429))
        ));
    }
    #[async_std::test]
    async fn malformed_json_is_not_accepted_as_an_empty_catalog() {
        let mut response = http_types::Response::new(http_types::StatusCode::Ok);
        response.set_body("not JSON");
        assert!(matches!(
            read_response::<serde_json::Value>(response.into()).await,
            Err(JsonRequestError::Decode(_))
        ));
    }
}
