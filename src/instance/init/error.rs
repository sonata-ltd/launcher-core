use thiserror::Error;

use crate::{
    data::db::DbError,
    instance::download::{
        artifacts::error::ArtifactError, assets::AssetSyncError, error::DownloadError,
        manifest::ManifestError,
    },
    modloader::error::ModloaderError,
    utils::files::FileError,
};

#[derive(Debug, Error)]
pub enum InitError {
    #[error(transparent)]
    Loader(#[from] ModloaderError),

    #[error(transparent)]
    Download(#[from] DownloadError),

    #[error(transparent)]
    File(#[from] FileError),

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    Db(#[from] DbError),

    #[error(transparent)]
    Manifest(#[from] ManifestError),

    #[error(transparent)]
    Assets(#[from] AssetSyncError),

    #[error(transparent)]
    Artifact(#[from] ArtifactError),
}

impl InitError {
    pub fn is_cancelled(&self) -> bool {
        match self {
            Self::Download(e) => e.is_cancelled(),
            Self::Loader(e) => e.is_cancelled(),
            Self::Artifact(e) => e.is_cancelled(),
            _ => false,
        }
    }
}

pub type Result<T> = std::result::Result<T, InitError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_direct_and_wrapped_cancellation() {
        assert!(InitError::from(DownloadError::Cancelled).is_cancelled());
        assert!(InitError::from(ModloaderError::Cancelled).is_cancelled());
        assert!(InitError::from(ModloaderError::from(DownloadError::Cancelled)).is_cancelled());
        assert!(InitError::from(ArtifactError::from(DownloadError::Cancelled)).is_cancelled());
    }

    #[test]
    fn failures_keep_their_types_and_are_not_cancellation() {
        let errors = [
            InitError::from(DbError::ResultCorrupted),
            InitError::from(FileError::InvalidPath("../file".into())),
            InitError::from(std::io::Error::other("write failed")),
            InitError::from(DownloadError::Http("503".into())),
            InitError::from(ModloaderError::Invalid("invalid profile".into())),
            InitError::from(ManifestError::ManifestMalformed("missing assets".into())),
            InitError::from(AssetSyncError::ManifestMalformed),
            InitError::from(ArtifactError::Invalid("invalid artifact".into())),
        ];
        for error in &errors {
            assert!(!error.is_cancelled(), "{error:?}");
        }
        assert!(matches!(errors[0], InitError::Db(DbError::ResultCorrupted)));
        assert!(matches!(
            errors[1],
            InitError::File(FileError::InvalidPath(_))
        ));
        assert!(matches!(errors[2], InitError::Io(_)));
        assert!(matches!(
            errors[3],
            InitError::Download(DownloadError::Http(_))
        ));
        assert!(matches!(
            errors[4],
            InitError::Loader(ModloaderError::Invalid(_))
        ));
        assert!(matches!(errors[5], InitError::Manifest(_)));
        assert!(matches!(errors[6], InitError::Assets(_)));
        assert!(matches!(
            errors[7],
            InitError::Artifact(ArtifactError::Invalid(_))
        ));
    }
}
