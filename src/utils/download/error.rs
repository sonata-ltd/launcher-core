use std::{io, path::PathBuf, time::Duration};

use http_types::url::ParseError;
use http_types::StatusCode;
use thiserror::Error;

pub type Result<T> = std::result::Result<T, DownloadError>;

type BoxError = Box<dyn std::error::Error + Send + Sync>;

#[derive(Debug, Error)]
pub enum DownloadError {
    #[error("request to {url} failed")]
    Request {
        url: String,
        #[source]
        source: BoxError,
    },

    #[error("got HTTP {status} when fetching {url}")]
    Http { status: StatusCode, url: String },

    #[error("malformed JSON in response from {url}")]
    MalformedJson {
        url: String,
        #[source]
        source: BoxError,
    },

    #[error("too many redirects ({count}) starting from {url}")]
    TooManyRedirects { url: String, count: usize },

    #[error("redirect {status} from {url} has no Location header")]
    RedirectWithoutLocation { status: StatusCode, url: String },

    #[error("cannot resolve redirect target `{location}` against {base}")]
    RedirectTargetInvalid {
        base: String,
        location: String,
        #[source]
        source: ParseError,
    },

    #[error("invalid url: {url}")]
    InvalidUrl {
        url: String,
        #[source]
        source: ParseError,
    },

    #[error("{} is not a valid save path", path.display())]
    InvalidSavePath { path: PathBuf },

    #[error("failed to create directory {}", path.display())]
    DirectoryCreation {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    #[error("failed to create file {}", path.display())]
    FileCreation {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    #[error("failed to write to {}", path.display())]
    FileWrite {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    #[error("failed to move {} into place at {}", from.display(), to.display())]
    FileCommit {
        from: PathBuf,
        to: PathBuf,
        #[source]
        source: io::Error,
    },

    #[error("failed to read response body of {url}")]
    BodyRead {
        url: String,
        #[source]
        source: io::Error,
    },

    #[error("timed out after {}s waiting for {phase} of {url}", after.as_secs())]
    Timeout {
        url: String,
        phase: &'static str,
        after: Duration,
    },

    #[error("checksum mismatch for {url}: expected {expected}, got {actual}")]
    ChecksumMismatch {
        url: String,
        expected: String,
        actual: String,
    },
}

impl DownloadError {
    pub fn is_transient(&self) -> bool {
        match self {
            Self::Request { .. } | Self::BodyRead { .. } | Self::Timeout { .. } => true,
            Self::Http { status, .. } => {
                status.is_server_error() || *status == StatusCode::TooManyRequests
            }
            Self::FileWrite { source, .. } | Self::FileCreation { source, .. } => {
                !matches!(source.kind(), io::ErrorKind::PermissionDenied)
            }
            _ => false,
        }
    }
}
