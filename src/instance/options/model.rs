use serde::{Deserialize, Serialize};
use strum::{Display, EnumString};

use crate::{
    instance::{
        model::InstanceId,
        settings::{EffectiveSettings, InstanceSettings},
    },
    java::model::JavaRuntime,
};

pub use crate::instance::settings::SettingsPatch;

#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, Display, EnumString,
)]
pub enum ExportType {
    #[default]
    Sonata,
    MultiMC,
    Modrinth,
}

#[derive(Debug, Clone, Serialize)]
pub struct Overview {
    pub name: String,
    pub tags: String,
    pub export_type: ExportType,
    pub playtime: i64,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OverviewPatch {
    pub name: Option<String>,
    pub tags: Option<String>,
    pub export_type: Option<ExportType>,
}

impl OverviewPatch {
    pub fn is_empty(&self) -> bool {
        self.name.is_none() && self.tags.is_none() && self.export_type.is_none()
    }

    pub fn has_fields(&self) -> bool {
        self.tags.is_some() || self.export_type.is_some()
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Settings {
    pub dir: String,
    pub overrides: InstanceSettings,
    pub effective: EffectiveSettings,
    pub java_runtime: Option<JavaRuntime>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "page", rename_all = "lowercase")]
pub enum PageView {
    Overview(Overview),
    Settings(Settings),
}

#[derive(Debug, Clone, Serialize)]
pub struct OptionUpdateMessage {
    pub instance_id: InstanceId,
    pub view: PageView,
}
