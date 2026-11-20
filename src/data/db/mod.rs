use sqlx::{
    migrate::{MigrateDatabase, MigrateError},
    Sqlite, SqlitePool,
};
use std::sync::Arc;
use thiserror::Error;

pub(crate) mod retention;
#[cfg(test)]
mod tests;

pub const DEFAULT_DB_NAME: &'static str = "cache.db";

#[derive(Debug, Clone)]
pub struct Database {
    pub pool: SqlitePool,
    pub(crate) layout_lease: Option<Arc<super::ownership::LayoutLease>>,
}

#[derive(Debug, Error)]
pub enum DbError {
    #[error("Database error: {0}")]
    Db(#[source] sqlx::Error),

    #[error("Conflict: {0}")]
    Conflict(#[source] sqlx::Error),

    #[error("foreign key violation: {0}")]
    ForeignKey(#[source] sqlx::Error),

    #[error("filesystem validation: {0}")]
    File(#[from] crate::utils::files::FileError),

    #[error("Invalid input: {0}")]
    InvalidInput(String),

    #[error("database is already bound to a different launcher resource layout")]
    LayoutConflict,

    #[error("Not found: {0}")]
    NotFound(String),

    #[error("Data corrupted")]
    ResultCorrupted,

    #[error("Insufficient data: {0}")]
    InsufficientData(String),

    #[error("Migration error: {0}")]
    MigrateError(#[source] MigrateError),
}

pub type Result<T> = std::result::Result<T, DbError>;

impl Database {
    pub(crate) async fn retention_owner(
        &self,
    ) -> std::result::Result<Option<crate::utils::object_store::catalog::DatabaseOwner>, sqlx::Error>
    {
        let (_, _, path): (i64, String, String) = sqlx::query_as("PRAGMA database_list")
            .fetch_all(&self.pool)
            .await?
            .into_iter()
            .find(|(_, name, _)| name == "main")
            .ok_or_else(|| sqlx::Error::Protocol("missing main database".into()))?;
        if path.is_empty() {
            return Ok(None);
        }
        let identity =
            sqlx::query_scalar("SELECT identity FROM retention_identity WHERE singleton=1")
                .fetch_one(&self.pool)
                .await?;
        Ok(Some(crate::utils::object_store::catalog::DatabaseOwner {
            identity,
            path: std::fs::canonicalize(path)?,
        }))
    }

    pub async fn init(url: &str) -> Result<Self> {
        tracing::info!(%url, "opening database");

        if !Sqlite::database_exists(url).await.unwrap_or(false) {
            Sqlite::create_database(url).await?;
        }

        let pool = SqlitePool::connect(url).await?;
        sqlx::migrate!("./migrations").run(&pool).await?;

        Ok(Self {
            pool,
            layout_lease: None,
        })
    }

    pub(crate) async fn init_owned(
        url: &str,
        paths: &super::layout::LauncherPaths,
        lease: Arc<super::ownership::LayoutLease>,
    ) -> Result<Self> {
        let key =
            super::ownership::layout_key(paths).map_err(crate::utils::files::FileError::from)?;
        let mut db = Self::init(url).await?;
        let mut tx = db.pool.begin_with("BEGIN IMMEDIATE").await?;
        sqlx::query(
            "INSERT INTO launcher_state(key,value) VALUES('database_layout_v1',?)
                     ON CONFLICT(key) DO NOTHING",
        )
        .bind(&key)
        .execute(&mut *tx)
        .await?;
        let stored: String =
            sqlx::query_scalar("SELECT value FROM launcher_state WHERE key='database_layout_v1'")
                .fetch_one(&mut *tx)
                .await?;
        if stored != key {
            return Err(DbError::LayoutConflict);
        }
        tx.commit().await?;
        db.layout_lease = Some(lease);
        Ok(db)
    }
}

impl From<sqlx::Error> for DbError {
    fn from(err: sqlx::Error) -> Self {
        match &err {
            sqlx::Error::Database(db_err) => {
                if db_err.is_unique_violation() {
                    DbError::Conflict(err)
                } else if db_err.is_foreign_key_violation() {
                    DbError::ForeignKey(err)
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
