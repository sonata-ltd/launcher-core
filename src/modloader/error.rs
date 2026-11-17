use thiserror::Error;

use crate::utils::{files::FileError, url::UrlBuildError};

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

    #[error(transparent)]
    Maven(#[from] crate::utils::maven::MavenCoordinateError),

    #[error(transparent)]
    Url(#[from] UrlBuildError),

    #[error(transparent)]
    File(#[from] FileError),

    #[error(transparent)]
    Download(#[from] crate::instance::download::error::DownloadError),

    #[error(transparent)]
    Rules(#[from] crate::instance::rules::RuleError),
}

impl ModloaderError {
    pub fn is_cancelled(&self) -> bool {
        match self {
            Self::Cancelled => true,
            Self::Download(error) => error.is_cancelled(),
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::instance::download::error::DownloadError;

    #[test]
    fn recognizes_processor_and_wrapped_download_cancellation() {
        assert!(ModloaderError::Cancelled.is_cancelled());
        assert!(ModloaderError::from(DownloadError::Cancelled).is_cancelled());
        assert!(!ModloaderError::from(DownloadError::Http("503".into())).is_cancelled());
        assert!(!ModloaderError::Invalid("invalid profile".into()).is_cancelled());
    }
}

pub type Result<T> = std::result::Result<T, ModloaderError>;
