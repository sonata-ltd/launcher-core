use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use sqlx::prelude::FromRow;

use crate::data::db::Database;
use crate::data::db::DbError::InsufficientData;
use crate::data::db::Result;
use crate::java::model::{JavaRuntime, JavaSource, NewJavaRuntime};
use crate::java::version::parse_major;

#[async_trait]
pub trait JavaStore: Send + Sync {
    async fn insert(&self, java: &NewJavaRuntime) -> Result<JavaRuntime>;
    /// All in one transaction. A runtime whose `exec_path` is already
    /// registered is skipped, not an error: the result holds only new rows.
    async fn insert_many(&self, javas: &[NewJavaRuntime]) -> Result<Vec<JavaRuntime>>;
    async fn get_by_id(&self, id: i64) -> Result<Option<JavaRuntime>>;
    async fn exists_by_path(&self, path: &PathBuf) -> Result<bool>;
    async fn list_all(&self) -> Result<Vec<JavaRuntime>>;
    /// Fills `major` for rows written before it existed, from `version`.
    async fn backfill_major(&self) -> Result<()>;
}

#[derive(Debug, FromRow)]
struct JavaRow {
    id: i64,
    version: String,
    major: i64,
    arch: Option<String>,
    exec_path: String,
    home_path: String,
    vendor: Option<String>,
    source: String,
}

/// A `NewJavaRuntime` with every required field present, ready for SQL.
struct Insertable<'a> {
    version: &'a str,
    major: i64,
    arch: Option<&'a str>,
    exec_path: String,
    home_path: String,
    vendor: Option<&'a str>,
    source: &'static str,
}

impl<'a> Insertable<'a> {
    fn from_new(new: &'a NewJavaRuntime) -> Result<Self> {
        let Some(version) = new.version.as_deref() else {
            return Err(InsufficientData("version is None".into()));
        };

        let Some(exec_path) = &new.exec_path else {
            return Err(InsufficientData("exec_path is None".into()));
        };

        let Some(home_path) = &new.home_path else {
            return Err(InsufficientData("home_path is None".into()));
        };

        let Some(major) = new.major.or_else(|| parse_major(version)) else {
            return Err(InsufficientData(format!(
                "unrecognized java version: {version}"
            )));
        };

        Ok(Self {
            version,
            major: i64::from(major),
            arch: new.arch.as_deref(),
            exec_path: exec_path.to_string_lossy().into_owned(),
            home_path: home_path.to_string_lossy().into_owned(),
            vendor: new.vendor.as_deref(),
            source: new.source.as_str(),
        })
    }
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
            major: u32::try_from(row.major).unwrap_or(0),
            arch: row.arch,
            exec_path: row.exec_path,
            home_path: row.home_path,
            vendor: row.vendor,
            source: JavaSource::parse(&row.source),
        }
    }
}

#[async_trait]
impl JavaStore for DbJavaStore {
    async fn insert(&self, new: &NewJavaRuntime) -> Result<JavaRuntime> {
        let row = Insertable::from_new(new)?;

        let rec = sqlx::query_as!(
            JavaRow,
            r#"
                INSERT INTO java_runtimes(
                    version,
                    major,
                    arch,
                    exec_path,
                    home_path,
                    vendor,
                    source
                )
                VALUES(?, ?, ?, ?, ?, ?, ?)
                RETURNING id, version, major, arch, exec_path, home_path, vendor, source
            "#,
            row.version,
            row.major,
            row.arch,
            row.exec_path,
            row.home_path,
            row.vendor,
            row.source
        )
        .fetch_one(&self.db.pool)
        .await?;

        Ok(Self::row_to_model(rec))
    }

    async fn insert_many(&self, javas: &[NewJavaRuntime]) -> Result<Vec<JavaRuntime>> {
        let mut tx = self.db.pool.begin().await?;
        let mut out = Vec::with_capacity(javas.len());

        for new in javas {
            let row = match Insertable::from_new(new) {
                Ok(row) => row,
                Err(e) => {
                    tracing::debug!(error = %e, "skipping incomplete java runtime");
                    continue;
                }
            };

            let rec = sqlx::query_as!(
                JavaRow,
                r#"
                    INSERT INTO java_runtimes(
                        version,
                        major,
                        arch,
                        exec_path,
                        home_path,
                        vendor,
                        source
                    )
                    VALUES(?, ?, ?, ?, ?, ?, ?)
                    ON CONFLICT(exec_path) DO NOTHING
                    RETURNING id, version, major, arch, exec_path, home_path, vendor, source
                "#,
                row.version,
                row.major,
                row.arch,
                row.exec_path,
                row.home_path,
                row.vendor,
                row.source
            )
            .fetch_optional(&mut *tx)
            .await?;

            if let Some(rec) = rec {
                out.push(Self::row_to_model(rec));
            }
        }

        tx.commit().await?;

        Ok(out)
    }

    async fn get_by_id(&self, id: i64) -> Result<Option<JavaRuntime>> {
        let rec = sqlx::query_as!(
            JavaRow,
            r#"
                SELECT id, version, major, arch, exec_path, home_path, vendor, source
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
                SELECT id, version, major, arch, exec_path, home_path, vendor, source
                FROM java_runtimes
                ORDER BY id
            "#
        )
        .fetch_all(&self.db.pool)
        .await?;

        Ok(rows.into_iter().map(Self::row_to_model).collect())
    }

    async fn backfill_major(&self) -> Result<()> {
        let rows = sqlx::query!("SELECT id, version FROM java_runtimes WHERE major = 0")
            .fetch_all(&self.db.pool)
            .await?;

        for row in rows {
            let Some(major) = parse_major(&row.version) else {
                tracing::warn!(id = row.id, version = %row.version, "cannot parse java major");
                continue;
            };
            let major = i64::from(major);

            sqlx::query!(
                "UPDATE java_runtimes SET major = ? WHERE id = ?",
                major,
                row.id
            )
            .execute(&self.db.pool)
            .await?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn store() -> DbJavaStore {
        let db = Database::init("sqlite::memory:")
            .await
            .expect("in-memory db");
        DbJavaStore::new(Arc::new(db))
    }

    fn new(exec: &str, version: &str) -> NewJavaRuntime {
        NewJavaRuntime {
            version: Some(version.into()),
            major: None,
            arch: Some("amd64".into()),
            exec_path: Some(PathBuf::from(exec)),
            home_path: Some(PathBuf::from(exec).with_file_name("")),
            vendor: None,
            source: JavaSource::Discovered,
        }
    }

    #[async_std::test]
    async fn insert_derives_major_from_version() {
        let store = store().await;

        let java = store
            .insert(&new("/a/bin/java", "1.8.0_402"))
            .await
            .unwrap();

        assert_eq!(java.major, 8);
        assert_eq!(java.source, JavaSource::Discovered);
        assert_eq!(java.arch.as_deref(), Some("amd64"));
    }

    #[async_std::test]
    async fn insert_many_skips_known_paths() {
        let store = store().await;
        store.insert(&new("/a/bin/java", "17.0.2")).await.unwrap();

        let added = store
            .insert_many(&[new("/a/bin/java", "17.0.2"), new("/b/bin/java", "21.0.5")])
            .await
            .unwrap();

        assert_eq!(added.len(), 1);
        assert_eq!(added[0].major, 21);
        assert_eq!(store.list_all().await.unwrap().len(), 2);
    }

    #[async_std::test]
    async fn backfill_fills_zero_majors() {
        let store = store().await;
        sqlx::query(
            "INSERT INTO java_runtimes(version, exec_path, home_path) VALUES ('21.0.5', '/x', '/')",
        )
        .execute(&store.db.pool)
        .await
        .unwrap();

        store.backfill_major().await.unwrap();

        assert_eq!(store.list_all().await.unwrap()[0].major, 21);
    }
}
