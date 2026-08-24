use std::sync::Arc;

use async_trait::async_trait;
use sqlx::{QueryBuilder, Sqlite};

use crate::{
    data::db::{Database, Result},
    instance::{
        model::InstanceId,
        settings::{GlobalSettings, InstanceSettings, SettingsPatch},
    },
};

#[async_trait]
pub trait InstanceSettingsStore: Send + Sync {
    async fn get(&self, id: InstanceId) -> Result<Option<InstanceSettings>>;
    async fn patch(&self, id: InstanceId, patch: &SettingsPatch) -> Result<bool>;
    async fn global(&self) -> Result<GlobalSettings>;
}

pub struct DbInstanceSettingsStore {
    db: Arc<Database>,
}

impl DbInstanceSettingsStore {
    pub fn new(db: Arc<Database>) -> Self {
        Self { db }
    }

    async fn exists(&self, id: InstanceId) -> Result<bool> {
        let exists: bool = sqlx::query_scalar!(
            r#"SELECT EXISTS(SELECT 1 FROM instances_settings WHERE instance_id = ?) AS "exists!: bool""#,
            id
        )
        .fetch_one(&self.db.pool)
        .await?;

        Ok(exists)
    }
}

#[async_trait]
impl InstanceSettingsStore for DbInstanceSettingsStore {
    async fn get(&self, id: InstanceId) -> Result<Option<InstanceSettings>> {
        let row = sqlx::query_as!(
            InstanceSettings,
            r#"
                SELECT
                    java_runtime AS "java_runtime?: i64",
                    memory_min   AS "memory_min?: i64",
                    memory_max   AS "memory_max?: i64",
                    jvm_args     AS "jvm_args?: String"
                FROM instances_settings
                WHERE instance_id = ?
            "#,
            id
        )
        .fetch_optional(&self.db.pool)
        .await?;

        Ok(row)
    }

    async fn patch(&self, id: InstanceId, patch: &SettingsPatch) -> Result<bool> {
        let mut qb: QueryBuilder<Sqlite> = QueryBuilder::new("UPDATE instances_settings SET ");
        let mut touched = false;

        {
            let mut set = qb.separated(", ");

            if let Some(value) = patch.java_runtime {
                set.push("java_runtime = ").push_bind_unseparated(value);
                touched = true;
            }

            if let Some(value) = patch.memory_min {
                set.push("memory_min = ").push_bind_unseparated(value);
                touched = true;
            }

            if let Some(value) = patch.memory_max {
                set.push("memory_max = ").push_bind_unseparated(value);
                touched = true;
            }

            if let Some(value) = patch.jvm_args.clone() {
                set.push("jvm_args = ").push_bind_unseparated(value);
                touched = true;
            }
        }

        if !touched {
            return self.exists(id).await;
        }

        qb.push(" WHERE instance_id = ").push_bind(id);

        let res = qb.build().execute(&self.db.pool).await?;

        Ok(res.rows_affected() > 0)
    }

    async fn global(&self) -> Result<GlobalSettings> {
        let row = sqlx::query_as!(
            GlobalSettings,
            r#"
                SELECT
                    java_runtime AS "java_runtime?: i64",
                    memory_min   AS "memory_min!: i64",
                    memory_max   AS "memory_max!: i64",
                    jvm_args     AS "jvm_args!: String"
                FROM settings_global
                WHERE id = 1
            "#
        )
        .fetch_optional(&self.db.pool)
        .await?;

        Ok(row.unwrap_or_default())
    }
}
