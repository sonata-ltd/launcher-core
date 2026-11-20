#[derive(Debug, thiserror::Error)]
pub enum JsonRequestError {
    #[error("HTTP {0}")]
    Status(u16),
    #[error("JSON request timed out")]
    Timeout,
    #[error("JSON response exceeds 8 MiB")]
    SizeLimit,
    #[error("{0}")]
    Transport(#[source] std::sync::Arc<dyn std::error::Error + Send + Sync>),
    #[error(transparent)]
    InvalidUrl(#[from] http_types::url::ParseError),
    #[error("unsupported URL scheme")]
    UnsupportedScheme,
    #[error(transparent)]
    Body(#[from] std::io::Error),
    #[error(transparent)]
    Decode(#[from] serde_json::Error),
}

impl JsonRequestError {
    pub fn is_transient(&self) -> bool {
        match self {
            Self::Status(status) => *status == 429 || (500..600).contains(status),
            Self::Timeout | Self::Transport(_) => true,
            Self::Body(error) => crate::utils::download::error::transient_io(error),
            _ => false,
        }
    }
}
