use serde::Deserialize;
use serde_json::value::RawValue;

use crate::{
    data::db::Database,
    instance::{
        options::pages::{
            overview::{Overview, OverviewFields},
            settings::{Settings, SettingsFields},
            Page, PageResult, ReadPage,
        },
        InstanceError,
    },
};

pub mod pages;
#[cfg(test)]
mod tests;

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
pub struct Options {
    overview: Overview,
}

#[derive(Debug)]
pub enum ChangableOptions {
    Overview(OverviewFields),
    Settings(SettingsFields)
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
            Page::Settings => {
                let page = Settings::from_db(id, &db).await?;
                Ok(PageResult::Settings(page))
            }
            _ => Err(InstanceError::NotImplemented),
        }
    }

    pub async fn change(db: &Database, request: ChangeRequest) -> Result<(), InstanceError> {
        match request.change {
            ChangableOptions::Overview(f) => {
                Overview::update(f, db, request.id).await?;
            },
            ChangableOptions::Settings(f) => {
                Settings::update(f, db, request.id).await?
            }
        }

        Ok(())
    }
}

#[derive(Deserialize, Debug)]
pub struct ChangeRequestBuilder {
    id: String,
    page: String,
    options: Box<RawValue>,
}

#[derive(Debug)]
pub struct ChangeRequest {
    id: i64,
    change: ChangableOptions,
}

impl ChangeRequestBuilder {
    pub fn build(self) -> Result<ChangeRequest, InstanceError> {
        let id: i64 = self.id.parse().map_err(|_| {
            InstanceError::WrongId(format!("must be integer (i64), got \"{}\"", self.id))
        })?;
        let page: Page = self.page.parse().map_err(|_| {
            InstanceError::OptionsPageWrong(self.page)
        })?;

        match page {
            Page::Overview => {
                let fields: OverviewFields = serde_json::from_str(&self.options.get()).map_err(|e| {
                    InstanceError::OptionNotAvailable(
                        format!("Failed to parse Overview page option: {}", e),
                    )
                })?;

                Ok(ChangeRequest { id, change: ChangableOptions::Overview(fields) })
            },
            Page::Settings => {
                let fields: SettingsFields = serde_json::from_str(&self.options.get()).map_err(|e| {
                    InstanceError::OptionNotAvailable(
                        format!("Failed to parse Settings page option: {}", e),
                    )
                })?;

                Ok(ChangeRequest { id, change: ChangableOptions::Settings(fields) })
            }
            _ => Err(InstanceError::NotImplemented)
        }
    }
}
