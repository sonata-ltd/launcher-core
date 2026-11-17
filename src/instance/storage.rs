use std::sync::Arc;

use async_trait::async_trait;

use crate::{
    data::db::{Database, Result},
    instance::model::{InstanceId, NewInstance},
    modloader::model::LoaderKind,
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
        let loader = new.loader.as_str();

        let rec = sqlx::query_as!(
            InstanceRecord,
            r#"
                INSERT INTO instances (name, dir, version, loader, loader_version, manifest_url, meta_provider)
                VALUES (?, ?, ?, ?, ?, ?, ?)
                RETURNING id AS "id: InstanceId",
                    name, dir, version, loader AS "loader: LoaderKind", loader_version, manifest_url, meta_provider
            "#,
            new.name,
            new.dir,
            new.version,
            loader,
            new.loader_version,
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
                name, dir, version, loader AS "loader: LoaderKind", loader_version, manifest_url, meta_provider FROM instances WHERE id = ?
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
            name, dir, version, loader AS "loader: LoaderKind", loader_version, manifest_url, meta_provider FROM instances ORDER BY id"#
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

#[cfg(test)]
mod tests {
    use super::*;

    #[async_std::test]
    async fn loader_kinds_roundtrip_through_instance_store() {
        let db = Arc::new(Database::init("sqlite::memory:").await.unwrap());
        let store = DbInstanceStore::new(Arc::clone(&db));

        for kind in LoaderKind::ALL {
            let new = NewInstance {
                name: kind.to_string(),
                dir: kind.as_str().to_owned(),
                version: "1.12.2".to_owned(),
                loader: kind,
                loader_version: (kind != LoaderKind::Vanilla).then(|| "fixture".to_owned()),
                manifest_url: "https://example.invalid/manifest.json".to_owned(),
                meta_provider: "prism".to_owned(),
            };
            let inserted = store.insert(&new).await.unwrap();
            assert_eq!(inserted.loader, kind);
            assert_eq!(inserted.loader_version, new.loader_version);
            let stored = store.get_by_id(inserted.id).await.unwrap().unwrap();
            assert_eq!(stored.loader, kind);
            assert_eq!(stored.loader_version, new.loader_version);
            assert_eq!(
                stored.to_scan_msg().data.info.unwrap().loader,
                kind.as_str()
            );

            let raw: String = sqlx::query_scalar("SELECT loader FROM instances WHERE id = ?")
                .bind(inserted.id)
                .fetch_one(&db.pool)
                .await
                .unwrap();
            assert_eq!(raw, kind.as_str());
        }

        let records = store.list_all().await.unwrap();
        assert_eq!(records.len(), LoaderKind::ALL.len());
        for (record, expected) in records.iter().zip(LoaderKind::ALL) {
            assert_eq!(record.loader, expected);
        }
    }

    #[async_std::test]
    async fn unknown_loader_is_rejected_during_decoding() {
        let db = Database::init("sqlite::memory:").await.unwrap();
        let error = sqlx::query_scalar::<_, LoaderKind>("SELECT 'unsupported'")
            .fetch_one(&db.pool)
            .await
            .unwrap_err();
        assert!(matches!(error, sqlx::Error::ColumnDecode { .. }));
    }
}
