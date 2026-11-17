use thiserror::Error;

use crate::{data::db::DbError, instance::model::InstanceId, java::error::JavaError};

#[derive(Debug, Error)]
pub enum OptionsError {
    #[error("instance not found: {0}")]
    InstanceNotFound(InstanceId),

    #[error("instance name is already taken")]
    NameTaken,

    #[error("unknown options page: {0}")]
    UnknownPage(String),

    #[error("java runtime not found: {0}")]
    JavaRuntimeNotFound(i64),

    #[error("malformed patch: {0}")]
    MalformedPatch(#[from] serde_json::Error),

    #[error("instance operation failed: {0}")]
    Internal(String),

    #[error(transparent)]
    Db(#[from] DbError),

    #[error(transparent)]
    Java(#[from] JavaError),
}

pub type Result<T> = std::result::Result<T, OptionsError>;
