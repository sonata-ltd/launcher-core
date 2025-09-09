use std::str::FromStr;

use serde::{Deserialize, Serialize};
use tide::utils::async_trait;
use ts_rs::TS;

use crate::{data::db::{DBError, Database}, instance::options::pages::{overview::Overview, settings::Settings}};

pub mod overview;
pub mod settings;


#[derive(Debug, Deserialize, Default, TS)]
#[allow(dead_code)]
pub struct General {
    id: String,
    version: String,
    loader: String
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

#[derive(Deserialize, Debug)]
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
#[serde(rename_all = "camelCase")]
pub enum PageResult {
    Overview(Overview),
    Mods(Mods),
    Worlds(Worlds),
    Resourcepacks(Resourcepacks),
    Shaderpacks(Shaderpacks),
    Logs(Logs),
    Settings(Settings),
}

#[async_trait]
pub trait ReadPage: Sized + Send  {
    async fn from_db(instance_id: i64, db: &Database) -> Result<Self, DBError>;
}
