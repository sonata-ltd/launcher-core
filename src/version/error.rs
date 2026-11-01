use crate::version::provider::MetaProvider;
use std::sync::Arc;
use thiserror::Error;

pub type Result<T> = std::result::Result<T, VersionError>;

#[derive(Debug, Error)]
pub enum VersionError {
    #[error("failed to download index: {0}")]
    IndexUnavailable(String),

    #[error("malformed index from {provider}")]
    MalformedIndex {
        provider: MetaProvider,
        #[source]
        source: serde_json::Error,
    },

    #[error("invalid version index: {0}")]
    InvalidIndex(String),

    #[error("version index returned HTTP {0}")]
    HttpStatus(u16),

    #[error("version index exceeds {0} bytes")]
    IndexTooLarge(usize),

    #[error("version catalog cache: {0}")]
    Cache(String),

    // Shared future must return a Clone result; preserve the original error
    // behind Arc rather than reducing it to a string for every consumer.
    #[error("{0}")]
    Shared(#[source] Arc<VersionError>),

    #[error("unknown version for provider {provider}, id {id}")]
    UnknownVersion { provider: String, id: String },

    #[error("meta provider `{0}` is not supported yet")]
    ProviderUnsupported(String),

    #[error("unknown meta provider: {0}")]
    UnknownProvider(String),
}
