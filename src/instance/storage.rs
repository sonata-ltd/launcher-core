use std::sync::Arc;

use async_trait::async_trait;

use crate::{
    data::db::{Database, Result},
    instance::model::{InstanceId, NewInstance},
};

use crate::instance::model::InstanceRecord;

#[async_trait]
pub trait InstanceStore: Send + Sync {
    async fn insert(&self, new: &NewInstance) -> Result<InstanceRecord>;
    async fn get_by_id(&self, id: InstanceId) -> Result<Option<InstanceRecord>>;
    async fn exists_by_dir(&self, dir: &str) -> Result<bool>;
    async fn list_all(&self) -> Result<Vec<InstanceRecord>>;
    async fn update_name(&self, id: InstanceId, new: &str) -> Result<()>;
    async fn delete(&self, id: InstanceId) -> Result<()>;
}

pub struct DbInstanceStore {
    db: Arc<Database>,
}

impl DbInstanceStore {
    pub fn new(db: Arc<Database>) -> Self {
        Self { db }
    }
}

#[async_trait]
impl InstanceStore for DbInstanceStore {
    async fn insert(&self, new: &NewInstance) -> Result<InstanceRecord> {
        let mut tx = self.db.pool.begin().await?;

        let rec = sqlx::query_as!(
            InstanceRecord,
            r#"
                INSERT INTO instances (name, dir, version, loader, manifest_url, meta_provider)
                VALUES (?, ?, ?, ?, ?, ?)
                RETURNING id AS "id: InstanceId",
                name, dir, version, loader, manifest_url, meta_provider
            "#,
            new.name,
            new.dir,
            new.version,
            new.loader,
            new.manifest_url,
            new.meta_provider
        )
        .fetch_one(&mut *tx)
        .await?;

        sqlx::query!(
            "INSERT INTO instances_overview (instance_id) VALUES (?)",
            rec.id
        )
        .execute(&mut *tx)
        .await?;

        sqlx::query!(
            "INSERT INTO instances_settings (instance_id) VALUES (?)",
            rec.id
        )
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;

        Ok(rec)
    }

    async fn get_by_id(&self, id: InstanceId) -> Result<Option<InstanceRecord>> {
        Ok(sqlx::query_as!(
            InstanceRecord,
            r#"
                SELECT id AS "id: InstanceId",
                name, dir, version, loader, manifest_url, meta_provider FROM instances WHERE id = ?
            "#,
            id
        )
        .fetch_optional(&self.db.pool)
        .await?)
    }

    async fn exists_by_dir(&self, dir: &str) -> Result<bool> {
        Ok(sqlx::query_scalar!(
            r#"SELECT EXISTS(SELECT 1 FROM instances WHERE dir = ?) as "exists!: bool""#,
            dir
        )
        .fetch_one(&self.db.pool)
        .await?)
    }

    async fn list_all(&self) -> Result<Vec<InstanceRecord>> {
        Ok(sqlx::query_as!(
            InstanceRecord,
            r#"SELECT id AS "id: InstanceId",
            name, dir, version, loader, manifest_url, meta_provider FROM instances ORDER BY id"#
        )
        .fetch_all(&self.db.pool)
        .await?)
    }

    async fn update_name(&self, id: InstanceId, new: &str) -> Result<()> {
        sqlx::query!(
            r#"UPDATE instances SET name = ?, updated_at = datetime('now') WHERE id = ?"#,
            new,
            id
        )
        .execute(&self.db.pool)
        .await?;
        Ok(())
    }

    async fn delete(&self, id: InstanceId) -> Result<()> {
        sqlx::query!(r#"DELETE FROM instances WHERE id = ?"#, id)
            .execute(&self.db.pool)
            .await?;
        Ok(())
    }
}
