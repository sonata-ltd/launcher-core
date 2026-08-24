use thiserror::Error;

use crate::version::provider::MetaProvider;

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

    #[error("unknown version for provider {provider}, id {id}")]
    UnknownVersion { provider: String, id: String },

    #[error("meta provider `{0}` is not supported yet")]
    ProviderUnsupported(String),

    #[error("unknown meta provider: {0}")]
    UnknownProvider(String),
}
