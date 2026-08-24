use std::str::FromStr;

use crate::instance::{
    model::InstanceId,
    options::{
        error::{OptionsError, Result},
        model::{OverviewPatch, SettingsPatch},
    },
};

use serde::Deserialize;
use serde_json::value::RawValue;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Overview,
    Settings,
}

impl FromStr for Page {
    type Err = OptionsError;

    fn from_str(s: &str) -> Result<Self> {
        match s.to_lowercase().as_str() {
            "overview" => Ok(Page::Overview),
            "settings" => Ok(Page::Settings),
            other => Err(OptionsError::UnknownPage(other.to_string())),
        }
    }
}

#[derive(Debug)]
pub enum PagePatch {
    Overview(OverviewPatch),
    Settings(SettingsPatch),
}

#[derive(Debug, Deserialize)]
pub struct ChangeRequestBody {
    id: i64,
    page: String,
    options: Box<RawValue>,
}

#[derive(Debug)]
pub struct ChangeRequest {
    pub id: InstanceId,
    pub patch: PagePatch,
}

impl ChangeRequestBody {
    pub fn build(self) -> Result<ChangeRequest> {
        let page: Page = self.page.parse()?;

        let patch = match page {
            Page::Overview => PagePatch::Overview(serde_json::from_str(self.options.get())?),
            Page::Settings => PagePatch::Settings(serde_json::from_str(self.options.get())?),
        };

        Ok(ChangeRequest {
            id: InstanceId::from(self.id),
            patch,
        })
    }
}
