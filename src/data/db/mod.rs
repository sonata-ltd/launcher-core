use std::{env, path::Path};

use sqlx::{
    migrate::{MigrateDatabase, MigrateError},
    Sqlite, SqlitePool,
};
use thiserror::Error;

pub const DEFAULT_DB_NAME: &'static str = "cache.db";

#[derive(Debug, Clone)]
pub struct Database {
    pub pool: SqlitePool,
}

#[derive(Debug, Error)]
pub enum DbError {
    #[error("Database error: {0}")]
    Db(#[source] sqlx::Error),

    #[error("Conflict: {0}")]
    Conflict(String),

    #[error("Invalid input: {0}")]
    InvalidInput(String),

    #[error("Not found: {0}")]
    NotFound(String),

    #[error("Data corrupted")]
    ResultCorrupted,

    #[error("Migration error: {0}")]
    MigrateError(#[source] MigrateError),
}

pub type Result<T> = std::result::Result<T, DbError>;

impl Database {
    pub async fn init(path: &Path) -> Result<Self> {
        let db_url = {
            let def = format!("sqlite://{}", path.display());

            if cfg!(debug_assertions) {
                dotenvy::dotenv_override().ok();

                match env::var("DATABASE_URL") {
                    Ok(val) => {
                        tracing::info!("using development database url: {}", val);
                        val
                    }
                    Err(e) => {
                        tracing::warn!(
                            "development environment detected, but database cannot be found: {}\nusing default path",
                            e.to_string()
                        );
                        def
                    }
                }
            } else {
                def
            }
        };

        tracing::info!("current database url: {}", db_url);

        if !Sqlite::database_exists(&db_url).await.unwrap_or(false) {
            Sqlite::create_database(&db_url).await?;
        }

        // `foreign_keys = ON` is a per-connection pragma, so running it once
        // against the pool would only cover whichever connection served that
        // query. sqlx already applies it to every connection it opens.
        let pool = SqlitePool::connect(&db_url).await?;
        sqlx::migrate!("./migrations").run(&pool).await?;

        Ok(Self { pool })
    }
}

impl From<sqlx::Error> for DbError {
    fn from(err: sqlx::Error) -> Self {
        match &err {
            sqlx::Error::Database(db_err) => {
                let msg = db_err.message().to_string();

                if msg.contains("UNIQUE constraint failed") || msg.contains("unique constraint") {
                    DbError::Conflict(msg)
                } else if msg.contains("FOREIGN KEY constraint failed")
                    || msg.contains("foreign key constraint")
                {
                    DbError::InvalidInput(msg)
                } else {
                    DbError::Db(err)
                }
            }

            sqlx::Error::RowNotFound => DbError::NotFound("Row not found".to_string()),
            _ => DbError::Db(err),
        }
    }
}

impl From<MigrateError> for DbError {
    fn from(err: MigrateError) -> Self {
        DbError::MigrateError(err)
    }
}
