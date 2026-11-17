#[derive(Debug, thiserror::Error)]
pub enum JsonRequestError {
    #[error("HTTP {0}")]
    Status(u16),
    #[error("JSON request timed out")]
    Timeout,
    #[error("JSON response exceeds 8 MiB")]
    SizeLimit,
    #[error("{0}")]
    Transport(String),
    #[error(transparent)]
    Decode(#[from] serde_json::Error),
}
