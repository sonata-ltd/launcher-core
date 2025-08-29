use serde::Deserialize;
use ts_rs::TS;

use crate::{data::db::Database, instance::{options::pages::{
    General, Overview, Page, PageResult, ReadPage,
}, InstanceError}};

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

    pub async fn retrieve(db: &Database, id: i64, page: Page) -> Result<PageResult, InstanceError> {
        match page {
            Page::Overview => {
                let page = Overview::from_db(id, &db).await?;
                Ok(PageResult::Overview(page))
            }
            _ => {
                Err(InstanceError::NotImplemented)
            }
        }
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
