use thiserror::Error;

use crate::data::db::DbError;

#[derive(Debug, Error)]
pub enum JavaError {
    #[error("Runtime already registered: {0}")]
    AlreadyRegistered(String),

    #[error("Database error: {0}")]
    Database(#[from] DbError),
}

pub type JavaResult<T> = std::result::Result<T, JavaError>;
