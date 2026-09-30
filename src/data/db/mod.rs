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
    pub async fn init(url: &str) -> Result<Self> {
        tracing::info!(%url, "opening database");

        if !Sqlite::database_exists(url).await.unwrap_or(false) {
            Sqlite::create_database(url).await?;
        }

        let pool = SqlitePool::connect(url).await?;
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
