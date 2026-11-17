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

impl DownloadError {
    pub fn is_cancelled(&self) -> bool {
        matches!(self, Self::Cancelled)
    }

    // Preserve the existing transfer policy: HTTP, timeout and I/O failures
    // are retried; cancellation and invalid content are terminal.
    pub(crate) fn is_retryable(&self) -> bool {
        matches!(
            self,
            Self::Http(_) | Self::Timeout(_) | Self::Io(_) | Self::File(_)
        )
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
    fn transport_and_filesystem_errors_keep_existing_retry_policy() {
        for error in [
            DownloadError::Http("503".into()),
            DownloadError::Timeout("response"),
            DownloadError::Io(std::io::Error::other("write failed")),
            DownloadError::File(FileError::Io(std::io::Error::other("commit failed"))),
        ] {
            assert!(error.is_retryable(), "{error:?}");
            assert!(!error.is_cancelled());
        }
        assert!(DownloadError::Cancelled.is_cancelled());
    }
}
