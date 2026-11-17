use crate::utils::files::FileError;
use thiserror::Error;

pub type Result<T> = std::result::Result<T, DownloadError>;

#[derive(Debug, Error)]
pub enum DownloadError {
    #[error("download was cancelled")]
    Cancelled,

    #[error("invalid download configuration or data: {0}")]
    Invalid(String),

    #[error("HTTP request failed: {0}")]
    Http(String),

    #[error(transparent)]
    Network(#[from] Box<super::super::error::DownloadError>),

    #[error("download timed out during {0}")]
    Timeout(&'static str),

    #[error("checksum mismatch for {url}: expected {expected}, got {actual}")]
    ChecksumMismatch {
        url: String,
        expected: String,
        actual: String,
    },

    #[error("download exceeds {limit} bytes")]
    SizeLimit { limit: u64 },

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    File(#[from] FileError),
}

impl From<crate::utils::cancellation::Cancelled> for DownloadError {
    fn from(_: crate::utils::cancellation::Cancelled) -> Self {
        Self::Cancelled
    }
}

impl DownloadError {
    pub fn is_cancelled(&self) -> bool {
        matches!(self, Self::Cancelled)
    }

    pub(crate) fn is_retryable(&self) -> bool {
        match self {
            Self::Network(error) => error.is_transient(),
            Self::Timeout(_) => true,
            Self::Io(error) | Self::File(FileError::Io(error)) => {
                super::super::error::transient_io(error)
            }
            _ => false,
        }
    }

    pub(crate) fn retry_delay(&self, fallback: std::time::Duration) -> std::time::Duration {
        match self {
            Self::Network(error) => error.retry_delay(fallback),
            _ => fallback,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancellation_and_invalid_content_are_not_retried() {
        for error in [
            DownloadError::Cancelled,
            DownloadError::Invalid("invalid SHA-1".into()),
            DownloadError::ChecksumMismatch {
                url: "https://example.invalid/file".into(),
                expected: "expected".into(),
                actual: "actual".into(),
            },
            DownloadError::SizeLimit { limit: 1024 },
        ] {
            assert!(!error.is_retryable(), "{error:?}");
        }
    }

    #[test]
    fn only_transient_transport_and_io_errors_are_retried() {
        for error in [
            DownloadError::Network(Box::new(super::super::super::error::DownloadError::Http {
                status: http_types::StatusCode::ServiceUnavailable,
                url: "https://example.invalid/file".into(),
            })),
            DownloadError::Timeout("response"),
            DownloadError::Io(std::io::ErrorKind::ConnectionReset.into()),
        ] {
            assert!(error.is_retryable(), "{error:?}");
            assert!(!error.is_cancelled());
        }
        assert!(!DownloadError::Io(std::io::ErrorKind::PermissionDenied.into()).is_retryable());
        assert!(!DownloadError::Io(std::io::ErrorKind::StorageFull.into()).is_retryable());
        assert!(DownloadError::Cancelled.is_cancelled());
    }
}
