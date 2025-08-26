use std::str::FromStr;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use ts_rs::TS;


#[derive(Debug, Deserialize, Default, TS)]
pub struct General {
    id: String,
    version: String,
    loader: String
}

#[derive(Debug, Deserialize, Serialize, Default, TS)]
pub struct Overview {
    name: String,
    tags: String,
    selected_export_type: Option<ExportTypes>,
    playtime: usize
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

pub trait ReadPage: for<'de> Deserialize<'de> + Sized {
    /// Parse from &str
    fn from_json_str(s: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(s)
    }

    /// Parse from JSON
    fn from_json_value(v: serde_json::Value) -> Result<Self, serde_json::Error> {
        serde_json::from_value(v)
    }

    /// Parse from Map
    fn from_json_ref(r: &Map<String, serde_json::Value>) -> Result<Self, serde_json::Error> {
        serde_json::from_value(Value::Object(r.to_owned()))
    }
}

impl ReadPage for Overview {}
impl ReadPage for Mods {}
impl ReadPage for Worlds {}
impl ReadPage for Resourcepacks {}
impl ReadPage for Shaderpacks {}
impl ReadPage for Logs {}
impl ReadPage for Settings {}
