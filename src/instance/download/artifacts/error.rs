use std::path::PathBuf;
use thiserror::Error;

use crate::{
    instance::{download::error::DownloadError, rules::RuleError},
    utils::{files::FileError, maven::MavenCoordinateError},
};

pub type Result<T> = std::result::Result<T, ArtifactError>;

#[derive(Debug, Error)]
pub enum ArtifactError {
    #[error("invalid artifact metadata: {0}")]
    Invalid(String),

    #[error("unsupported artifact metadata: {0}")]
    Unsupported(String),

    #[error("checksum mismatch for {}: expected {expected}, got {actual}", path.display())]
    ChecksumMismatch {
        path: PathBuf,
        expected: String,
        actual: String,
    },

    #[error(transparent)]
    Download(#[from] DownloadError),

    #[error(transparent)]
    File(#[from] FileError),

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    Maven(#[from] MavenCoordinateError),

    #[error(transparent)]
    Rules(#[from] RuleError),
}

impl ArtifactError {
    pub fn is_cancelled(&self) -> bool {
        matches!(self, Self::Download(error) if error.is_cancelled())
    }
}
