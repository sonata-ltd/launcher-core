use thiserror::Error;

#[derive(Debug, Error)]
pub enum ModloaderError {
    #[error("invalid loader configuration: {0}")]
    Invalid(String),

    #[error("unsupported installer/profile format: {0}")]
    Unsupported(String),

    #[error("metadata request failed: {0}")]
    Http(String),

    #[error("processor failed: {0}")]
    Processor(String),

    #[error("installation was cancelled")]
    Cancelled,

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, ModloaderError>;
