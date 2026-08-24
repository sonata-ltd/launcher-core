use crate::{
    data::db::DbError,
    instance::{launch::LaunchError, model::InstanceId},
    java::error::JavaError,
    version::error::VersionError,
};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum InstanceError {
    #[error("Failed to create new instance: {0}")]
    CreationFailed(String),

    #[error("Failed to run instance: {0}")]
    RunFailed(String),

    #[error("rename failed: {0}")]
    RenameFailed(String),

    #[error("Instance id not found: {0}")]
    NotFound(InstanceId),

    #[error("Instance {0} has no linked libraries, install it first")]
    NotInstalled(InstanceId),

    #[error("Failed to register instance: {0}")]
    RegistrationFailed(String),

    #[error("Name of the instance is already taken")]
    NameTaken,

    #[error("Failed to generate manifest for instance: {0}")]
    ManifestGenerationFailed(String),

    #[error("Failed to create instance directory: {0}")]
    DirCreationFailed(String),

    #[error("Failed to get home direcory")]
    HomeDirNotFound,

    #[error("Paths is not initialized")]
    PathsNotInitialized,

    #[error("Failed to retrieve instance version")]
    VersionNotAvailable,

    #[error("No java runtime selected for instance {0}")]
    JavaRuntimeNotSelected(InstanceId),

    #[error("Java runtime {0} is not registered or no longer exists")]
    JavaRuntimeMissing(i64),

    #[error("Failed to convert into JSON: {0}")]
    JSONConstructionFailed(#[from] serde_json::Error),

    #[error(transparent)]
    Java(#[from] JavaError),

    #[error(transparent)]
    Version(#[from] VersionError),

    #[error(transparent)]
    Launch(#[from] LaunchError),

    #[error(transparent)]
    DB(#[from] DbError),
}

pub type Result<T> = std::result::Result<T, InstanceError>;
