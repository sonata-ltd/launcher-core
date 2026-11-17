//! Общий ограниченный JSON transport. Здесь нет знаний о модах и провайдерах.
use futures::AsyncReadExt;
use std::time::Duration;

const MAX_JSON_BYTES: usize = 8 * 1024 * 1024;
use serde::de::DeserializeOwned;

/// Ленивый общий клиент. Config::try_into возвращает ошибку инициализации,
/// в отличие от Client::new, который у curl backend может вызвать panic.
#[derive(Default)]
pub struct JsonClient {
    client: std::sync::OnceLock<Result<surf::Client, String>>,
}

impl JsonClient {
    pub fn get(&self, url: surf::Url) -> Result<surf::RequestBuilder, JsonRequestError> {
        let client = self
            .client
            .get_or_init(|| {
                surf::Client::try_from(surf::Config::new()).map_err(|error| error.to_string())
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
    async_std::future::timeout(Duration::from_secs(25), async {
        let response = request
            .send()
            .await
            .map_err(|error| JsonRequestError::Transport(error.to_string()))?;
        read_response(response).await
    })
    .await
    .map_err(|_| JsonRequestError::Timeout)?
}

/// Проверяем ответ отдельно от получения HTTP-соединения. Так лимит тела,
/// status и JSON можно проверить настоящим Response без локального сервера.
pub async fn read_response<T: DeserializeOwned>(
    mut response: surf::Response,
) -> Result<T, JsonRequestError> {
    if !response.status().is_success() {
        return Err(JsonRequestError::Status(u16::from(response.status())));
    }
    let mut bytes = Vec::new();
    response
        .take_body()
        .into_reader()
        .take((MAX_JSON_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .await
        .map_err(|error| JsonRequestError::Transport(error.to_string()))?;
    if bytes.len() > MAX_JSON_BYTES {
        return Err(JsonRequestError::SizeLimit);
    }
    Ok(serde_json::from_slice(&bytes)?)
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
