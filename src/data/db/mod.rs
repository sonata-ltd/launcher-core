use std::path::Path;

use sqlx::{migrate::{MigrateDatabase, MigrateError}, Sqlite, SqlitePool};
use thiserror::Error;

#[derive(Debug, Clone)]
pub struct Database {
    pub pool: SqlitePool
}

#[derive(Debug, Error)]
pub enum DBError {
    #[error("Database error: {0}")]
    Db(#[source] sqlx::Error),

    #[error("Conflict: {0}")]
    Conflict(String),

    #[error("Invalid input: {0}")]
    InvalidInput(String),

    #[error("Not found: {0}")]
    NotFound(String),

    #[error("Migration error: {0}")]
    MigrateError(#[source] MigrateError)
}

pub type Result<T> = std::result::Result<T, DBError>;

impl Database {
    pub async fn init(path: &Path) -> Result<Self> {
        let db_url = format!("sqlite://{}", path.display());

        if !Sqlite::database_exists(&db_url).await.unwrap_or(false) {
            Sqlite::create_database(&db_url).await?;
        }

        let pool = SqlitePool::connect(&db_url).await?;
        sqlx::migrate!("./migrations").run(&pool).await?;
        sqlx::query("PRAGMA foreign_keys = ON;").execute(&pool).await?;

        Ok(Self { pool })
    }
}

impl From<sqlx::Error> for DBError {
    fn from(err: sqlx::Error) -> Self {
        match &err {
            sqlx::Error::Database(db_err) => {
                let msg = db_err.message().to_string();

                if msg.contains("UNIQUE constraint failed") || msg.contains("unique constraint") {
                    DBError::Conflict(msg)
                } else if msg.contains("FOREIGN KEY constraint failed")
                    || msg.contains("foreign key constraint")
                {
                    DBError::InvalidInput(msg)
                } else {
                    DBError::Db(err)
                }
            }

            sqlx::Error::RowNotFound => DBError::NotFound("Row not found".to_string()),
            _ => DBError::Db(err)
        }
    }
}

impl From<MigrateError> for DBError {
    fn from(err: MigrateError) -> Self {
        DBError::MigrateError(err)
    }
}
