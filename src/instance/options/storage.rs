use std::sync::Arc;

use async_trait::async_trait;
use sqlx::prelude::FromRow;

use crate::data::db::{Database, DbError, Result};
use crate::instance::options::model::{ExportType, OverviewPatch};
use crate::instance::{model::InstanceId, options::model::Overview};

#[async_trait]
pub trait OptionsStore: Send + Sync {
    async fn overview(&self, id: InstanceId) -> Result<Option<Overview>>;
    async fn patch_overview(&self, id: InstanceId, patch: &OverviewPatch) -> Result<bool>;
}

pub struct DbOptionsStore {
    db: Arc<Database>,
}

impl DbOptionsStore {
    pub fn new(db: Arc<Database>) -> Self {
        Self { db }
    }
}

#[derive(Debug, FromRow)]
struct OverviewRow {
    name: String,
    tags: String,
    export_type: String,
    playtime: i64,
}

#[async_trait]
impl OptionsStore for DbOptionsStore {
    async fn overview(&self, id: InstanceId) -> Result<Option<Overview>> {
        let Some(row) = sqlx::query_as!(
            OverviewRow,
            r#"
                SELECT i.name, o.tags, o.export_type, o.playtime
                FROM instances_overview o
                JOIN instances i ON i.id = o.instance_id
                WHERE o.instance_id = ?
            "#,
            id
        )
        .fetch_optional(&self.db.pool)
        .await?
        else {
            return Ok(None);
        };

        let export_type: ExportType = row
            .export_type
            .parse()
            .map_err(|_| DbError::ResultCorrupted)?;

        Ok(Some(Overview {
            name: row.name,
            tags: row.tags,
            export_type,
            playtime: row.playtime,
        }))
    }

    async fn patch_overview(&self, id: InstanceId, patch: &OverviewPatch) -> Result<bool> {
        let export_type = patch.export_type.map(|e| e.to_string());

        let res = sqlx::query!(
            r#"
                UPDATE instances_overview
                SET tags        = COALESCE(?1, tags),
                    export_type = COALESCE(?2, export_type)
                    WHERE instance_id = ?3
            "#,
            patch.tags,
            export_type,
            id
        )
        .execute(&self.db.pool)
        .await?;

        Ok(res.rows_affected() > 0)
    }
}
