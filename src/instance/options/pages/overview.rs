use std::fmt::Display;
use std::str::FromStr;

use serde::Deserialize;
use serde::Serialize;
use tide::utils::async_trait;
use ts_rs::TS;

use crate::data::db::DBError;
use crate::data::db::Database;
use crate::data::db::Result;

#[derive(Debug, Deserialize, Serialize, Default)]
pub struct Overview {
    name: String,
    tags: String,
    export_type: ExportTypes,
    playtime: i64,
}

#[derive(Debug, Deserialize, Serialize, Default, PartialEq, TS)]
#[ts(export_to = "./options/overview/")]
pub enum ExportTypes {
    #[default]
    Sonata,
    MultiMC,
    Modrinth
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Deserialize, Serialize, Default, TS)]
#[ts(export_to = "./options/overview/")]
#[serde(deny_unknown_fields)]
pub struct OverviewFields {
    pub name: Option<String>,
    pub tags: Option<String>,
    pub export_type: Option<ExportTypes>,
    pub playtime: Option<i64>,
}

impl From<Overview> for OverviewFields {
    fn from(s: Overview) -> Self {
        OverviewFields {
            name: Some(s.name),
            tags: Some(s.tags),
            export_type: Some(s.export_type),
            playtime: Some(s.playtime),
        }
    }
}

impl Overview {
    pub fn new(name: String, tags: String, export_type: ExportTypes, playtime: i64) -> Self {
        Self {
            name,
            tags,
            export_type,
            playtime,
        }
    }

    pub async fn update(change: OverviewFields, db: &Database, instance_id: i64) -> Result<()> {
        let export_type = match change.export_type {
            Some(export) => Some(export.to_string()),
            None => None,
        };

        sqlx::query!(
            r#"
            UPDATE instances_overview
            SET
                name = COALESCE($1, name),
                tags = COALESCE($2, tags),
                export_type = COALESCE($3, export_type),
                playtime = COALESCE($4, playtime)
            WHERE instance_id = $5
            "#,
            change.name,
            change.tags,
            export_type,
            change.playtime,
            instance_id
        )
        .execute(&db.pool)
        .await?;

        Ok(())
    }

    pub async fn insert(&self, db: &Database, instance_id: i64) -> Result<()> {
        let export_type_string = self.export_type.to_string();

        sqlx::query!(
            r#"
            INSERT INTO instances_overview (instance_id, name, tags, export_type, playtime)
            VALUES (?1, ?2, ?3, ?4, ?5)
            "#,
            instance_id,
            self.name,
            self.tags,
            export_type_string,
            self.playtime,
        )
        .execute(&db.pool)
        .await?;

        Ok(())
    }
}

#[async_trait]
impl super::ReadPage for Overview {
    async fn from_db(instance_id: i64, db: &Database) -> Result<Self> {
        let rec = sqlx::query!(
            r#"
            SELECT name, tags, export_type, playtime
            FROM instances_overview
            WHERE instance_id = ?
            "#,
            instance_id
        )
        .fetch_one(&db.pool)
        .await?;

        let export_type: ExportTypes = rec.export_type.parse()?;
        let page = Overview {
            name: rec.name,
            tags: rec.tags,
            export_type,
            playtime: rec.playtime,
        };

        Ok(page)
    }
}

impl FromStr for ExportTypes {
    type Err = DBError;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        let res = match s {
            "Sonata" => ExportTypes::Sonata,
            "MultiMC" => ExportTypes::MultiMC,
            "Modrinth" => ExportTypes::Modrinth,
            _ => return Err(DBError::ResultCorrupted),
        };

        Ok(res)
    }
}

impl Display for ExportTypes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self)
    }
}
