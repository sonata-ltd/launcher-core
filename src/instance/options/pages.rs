use std::str::FromStr;

use serde::{Deserialize, Serialize};
use tide::utils::async_trait;
use ts_rs::TS;

use crate::data::db::{DBError, Database};


#[derive(Debug, Deserialize, Default, TS)]
#[allow(dead_code)]
pub struct General {
    id: String,
    version: String,
    loader: String
}

#[derive(Debug, Deserialize, Serialize, Default, TS)]
pub struct Overview {
    name: String,
    tags: String,
    // selected_export_type: Option<ExportTypes>,
    // playtime: usize
}

#[derive(Debug, Deserialize, Serialize, Default)]
pub struct Mods {}

#[derive(Debug, Deserialize, Serialize, Default)]
pub struct Worlds {}

#[derive(Debug, Deserialize, Serialize, Default)]
pub struct Resourcepacks {}

#[derive(Debug, Deserialize, Serialize, Default)]
pub struct Shaderpacks {}

#[derive(Debug, Deserialize, Serialize, Default)]
pub struct Logs {}

#[derive(Debug, Deserialize, Serialize, Default)]
pub struct Settings {}

#[derive(Debug, Deserialize, Serialize, Default, TS)]
pub enum ExportTypes {
    #[default]
    Sonata,
    Prism
}

#[derive(Deserialize)]
pub enum Page {
    Overview,
    Mods,
    Worlds,
    Resourcepacks,
    Shaderpacks,
    Logs,
    Settings
}

#[derive(Debug)]
pub struct ParsePageError;

impl FromStr for Page {
    type Err = ParsePageError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "overview" => Ok(Page::Overview),
            "mods" => Ok(Page::Mods),
            "worlds" => Ok(Page::Worlds),
            "resourcepacks" => Ok(Page::Resourcepacks),
            "shaderpacks" => Ok(Page::Shaderpacks),
            "logs" => Ok(Page::Logs),
            "settings" => Ok(Page::Settings),
            _ => Err(ParsePageError)
        }
    }
}

#[derive(Serialize)]
pub enum PageResult {
    Overview(Overview),
    Mods(Mods),
    Worlds(Worlds),
    Resourcepacks(Resourcepacks),
    Shaderpacks(Shaderpacks),
    Logs(Logs),
    Settings(Settings)
}

#[async_trait]
pub trait ReadPage: Sized + Send  {
    async fn from_db(instance_id: i64, db: &Database) -> Result<Self, DBError>;
}

#[async_trait]
impl ReadPage for Overview {
    async fn from_db(instance_id: i64, db: &Database) -> Result<Self, DBError> {
        let rec = sqlx::query_as!(
            Overview,
            r#"
            SELECT name, tags
            FROM instances_overview
            WHERE instance_id = ?
            "#,
            instance_id
        ).fetch_optional(&db.pool)
        .await?;

        let page = rec.ok_or_else(|| DBError::NotFound(format!("Overview page not found")))?;
        Ok(page)
    }
}
