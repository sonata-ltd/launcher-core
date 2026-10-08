use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use sqlx::prelude::FromRow;

use crate::data::db::Database;
use crate::data::db::DbError::InsufficientData;
use crate::data::db::Result;
use crate::java::model::JavaRuntime;
use crate::java::model::NewJavaRuntime;

#[async_trait]
pub trait JavaStore: Send + Sync {
    async fn insert(&self, java: &NewJavaRuntime) -> Result<JavaRuntime>;
    async fn get_by_id(&self, id: i64) -> Result<Option<JavaRuntime>>;
    async fn exists_by_path(&self, path: &PathBuf) -> Result<bool>;
    async fn list_all(&self) -> Result<Vec<JavaRuntime>>;
}

#[derive(Debug, FromRow)]
struct JavaRow {
    id: i64,
    version: String,
    exec_path: String,
    home_path: String,
    vendor: Option<String>,
}

pub struct DbJavaStore {
    db: Arc<Database>,
}

impl DbJavaStore {
    pub fn new(db: Arc<Database>) -> Self {
        Self { db }
    }

    fn row_to_model(row: JavaRow) -> JavaRuntime {
        JavaRuntime {
            id: row.id,
            version: row.version,
            exec_path: row.exec_path,
            home_path: row.home_path,
            vendor: row.vendor,
        }
    }
}

#[async_trait]
impl JavaStore for DbJavaStore {
    async fn insert(&self, new: &NewJavaRuntime) -> Result<JavaRuntime> {
        let NewJavaRuntime {
            version,
            exec_path,
            home_path,
            vendor,
        } = new;

        let Some(version) = version else {
            return Err(InsufficientData(format!("version is None")));
        };

        let Some(exec_path) = exec_path else {
            return Err(InsufficientData(format!("exec_path is None")));
        };

        let exec_path = exec_path.to_str();

        let Some(home_path) = home_path else {
            return Err(InsufficientData(format!("home_path is None")));
        };

        let home_path = home_path.to_str();

        let java = sqlx::query_as!(
            JavaRuntime,
            r#"
                INSERT INTO java_runtimes(
                    version,
                    exec_path,
                    home_path,
                    vendor
                )
                VALUES(?, ?, ?, ?)
                RETURNING id, version, exec_path, home_path, vendor
            "#,
            version,
            exec_path,
            home_path,
            vendor
        )
        .fetch_one(&self.db.pool)
        .await?;

        Ok(java)
    }

    async fn get_by_id(&self, id: i64) -> Result<Option<JavaRuntime>> {
        let rec = sqlx::query_as!(
            JavaRow,
            r#"
                SELECT id, version, exec_path, home_path, vendor
                FROM java_runtimes
                WHERE id = ?
            "#,
            id
        )
        .fetch_optional(&self.db.pool)
        .await?;

        Ok(rec.map(Self::row_to_model))
    }

    async fn exists_by_path(&self, path: &PathBuf) -> Result<bool> {
        let path = path.as_path().to_string_lossy();

        let exists: bool = sqlx::query_scalar!(
            r#"SELECT EXISTS(SELECT 1 FROM java_runtimes WHERE exec_path = ?) as "exists!: bool""#,
            path
        )
        .fetch_one(&self.db.pool)
        .await?;

        Ok(exists)
    }

    async fn list_all(&self) -> Result<Vec<JavaRuntime>> {
        let rows = sqlx::query_as!(
            JavaRow,
            r#"
                SELECT id, version, exec_path, home_path, vendor
                FROM java_runtimes
                ORDER BY id
            "#
        )
        .fetch_all(&self.db.pool)
        .await?;

        Ok(rows.into_iter().map(Self::row_to_model).collect())
    }
}
