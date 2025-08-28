use serde::{ser::Error, Deserialize};
use ts_rs::TS;

use crate::instance::options::pages::{
    General, Logs, Mods, Overview, Page, PageResult, ReadPage, Resourcepacks, Settings,
    Shaderpacks, Worlds,
};

pub mod pages;

#[derive(Debug, Deserialize, Default, TS)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
pub struct Options {
    general: General,
    overview: Overview,
}

impl Options {
    pub fn new() {
        let data = Self::default();
        println!("{:#?}", data);
    }

    pub fn retrieve_all(json: serde_json::Value) -> Result<Self, serde_json::Error> {
        let root: Options = serde_json::from_value(json)?;
        Ok(root)
    }

    pub fn retrieve(page: Page, json: serde_json::Value) -> Result<PageResult, serde_json::Error> {
        match page {
            Page::Overview => {
                if let Some(overview) = json.get("overview").and_then(|v| v.as_object()) {
                    let v = Overview::from_json_ref(overview)?;
                    return Ok(PageResult::Overview(v));
                }
            }
            Page::Mods => {
                let v = Mods::from_json_value(json)?;
                return Ok(PageResult::Mods(v));
            }
            Page::Worlds => {
                let v = Worlds::from_json_value(json)?;
                return Ok(PageResult::Worlds(v));
            }
            Page::Resourcepacks => {
                let v = Resourcepacks::from_json_value(json)?;
                return Ok(PageResult::Resourcepacks(v));
            }
            Page::Shaderpacks => {
                let v = Shaderpacks::from_json_value(json)?;
                return Ok(PageResult::Shaderpacks(v));
            }
            Page::Logs => {
                let v = Logs::from_json_value(json)?;
                return Ok(PageResult::Logs(v));
            }
            Page::Settings => {
                let v = Settings::from_json_value(json)?;
                return Ok(PageResult::Settings(v));
            }
        }

        Err(serde_json::Error::custom("Page not found in the manifest"))
    }
}


#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    #[test]
    fn export_bindings() -> Result<(), Box<dyn std::error::Error>> {
        let out = concat!(env!("CARGO_MANIFEST_DIR"), "/bindings/instance/options");
        fs::create_dir_all(out)?;
        Options::export_all_to(out)?;
        Ok(())
    }
}
