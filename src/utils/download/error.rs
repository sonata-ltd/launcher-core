use std::{io, path::PathBuf, time::Duration};

use http_types::url::ParseError;
use http_types::StatusCode;
use thiserror::Error;

pub type Result<T> = std::result::Result<T, DownloadError>;

type BoxError = Box<dyn std::error::Error + Send + Sync>;

#[derive(Debug, Error)]
pub enum DownloadError {
    #[error(transparent)]
    Cancelled(#[from] crate::utils::cancellation::Cancelled),

    #[error("unsupported URL scheme for {url} (HTTPS required: {https_only})")]
    UnsupportedScheme { url: String, https_only: bool },

    #[error("credentials embedded in download URL {url} are unsupported")]
    UrlCredentials { url: String },

    #[error("download redirect cannot change the origin over HTTP: {url}")]
    InsecureRedirect { url: String },

    #[error("expected {expected} bytes, received {actual}")]
    SizeMismatch { expected: u64, actual: u64 },
    #[error("request to {url} failed")]
    Request {
        url: String,
        #[source]
        source: BoxError,
    },

    #[error("got HTTP {status} when fetching {url}")]
    Http { status: StatusCode, url: String },

    #[error("response from {url} exceeds {limit} bytes")]
    SizeLimit { url: String, limit: u64 },

    #[error("got HTTP {status} when fetching {url}; retry after {}s", after.as_secs())]
    RetryAfter {
        status: StatusCode,
        url: String,
        after: Duration,
    },

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

pub(super) fn transient_status(status: StatusCode) -> bool {
    status.is_server_error() || status == StatusCode::TooManyRequests
}

pub(crate) fn transient_io(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::ConnectionAborted
            | io::ErrorKind::ConnectionReset
            | io::ErrorKind::BrokenPipe
            | io::ErrorKind::TimedOut
            | io::ErrorKind::WouldBlock
    )
}

impl DownloadError {
    pub fn is_transient(&self) -> bool {
        match self {
            Self::Request { .. } | Self::Timeout { .. } => true,
            Self::Http { status, .. } | Self::RetryAfter { status, .. } => {
                transient_status(*status)
            }
            Self::BodyRead { source, .. } | Self::FileWrite { source, .. } => transient_io(source),
            _ => false,
        }
    }

    pub(crate) fn retry_delay(&self, fallback: Duration) -> Duration {
        match self {
            Self::RetryAfter { after, .. } => *after,
            _ => fallback,
        }
    }
}
