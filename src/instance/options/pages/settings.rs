use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tide::utils::async_trait;
use ts_rs::TS;

use crate::{
    data::db::{DBError, Database, Result},
    instance::options::pages::ReadPage,
};

#[derive(Debug, Serialize)]
pub struct Settings {
    dir: PathBuf,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Deserialize, Serialize, Default, TS)]
#[serde(deny_unknown_fields)]
#[ts(export_to = "./options/settings/")]
pub struct SettingsFields {
    pub dir: Option<PathBuf>,
}

impl From<Settings> for SettingsFields {
    fn from(s: Settings) -> Self {
        SettingsFields { dir: Some(s.dir) }
    }
}

impl Settings {
    pub async fn update(change: SettingsFields, db: &Database, instance_id: i64) -> Result<()> {
        let dir = match change.dir {
            Some(dir) => Some(dir.display().to_string()),
            None => None
        };

        sqlx::query!(
            r#"
            UPDATE instances_settings
            SET
                dir = COALESCE($1, dir)
            WHERE instance_id = $2
            "#,
            dir,
            instance_id
        ).execute(&db.pool).await?;

        Ok(())
    }

    pub async fn upset<P: AsRef<Path>>(db: &Database, instance_id: i64, dir: &P) -> Result<()> {
        let dir = dir.as_ref().to_str();

        sqlx::query!(
            r#"
            INSERT INTO instances_settings (instance_id, dir)
            VALUES (?1, ?2)
            ON CONFLICT(instance_id) DO UPDATE SET
                dir = excluded.dir
            "#,
            instance_id,
            dir
        )
        .execute(&db.pool)
        .await?;

        Ok(())
    }
}

#[async_trait]
impl ReadPage for Settings {
    async fn from_db(instance_id: i64, db: &Database) -> Result<Self> {
        let rec = sqlx::query_as!(
            Settings,
            r#"
                SELECT dir
                FROM instances_settings
                WHERE instance_id = ?
                "#,
            instance_id
        )
        .fetch_optional(&db.pool)
        .await?;

        let settings = rec.ok_or_else(|| DBError::NotFound(format!("Overview page not found")))?;
        Ok(settings)
    }
}
