use std::path::Path;

use sqlx::{migrate::MigrateDatabase, Sqlite, SqlitePool};

#[derive(Debug, Clone)]
pub struct Database {
    pub pool: SqlitePool
}

impl Database {
    pub async fn init(path: &Path) -> Result<Self, sqlx::Error> {
        let db_url = format!("sqlite://{}", path.display());

        if !Sqlite::database_exists(&db_url).await.unwrap_or(false) {
            Sqlite::create_database(&db_url).await?;
        }

        let pool = SqlitePool::connect(&db_url).await?;
        sqlx::migrate!("./migrations").run(&pool).await?;

        Ok(Self { pool })
    }
}
