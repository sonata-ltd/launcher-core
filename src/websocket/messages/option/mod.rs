use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use ts_rs::TS;

use crate::{instance::options::pages::{overview::OverviewFields, settings::SettingsFields}, websocket::messages::BaseMessage};

#[derive(Debug, Serialize, Deserialize, TS)]
#[ts(export_to = "./options/")]
pub struct OptionUpdateMessage {
    pub base: BaseMessage,
    pub option: InstanceFields,
}

#[derive(Debug, TS)]
pub enum InstanceFields {
    Overview(OverviewFields),
    Settings(SettingsFields)
}


impl Serialize for InstanceFields {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer
    {
        use serde::ser::SerializeMap;

        let mut map = serializer.serialize_map(None)?;

        match self {
            InstanceFields::Overview(fields) => {
                map.serialize_entry("page", "overview")?;

                let v = serde_json::to_value(fields).map_err(serde::ser::Error::custom)?;
                if let Value::Object(obj) = v {
                    for (k, v) in obj {
                        // Skip fields not present
                        map.serialize_entry(&k, &v)?;
                    }
                }
            },
            InstanceFields::Settings(fields) => {
                map.serialize_entry("page", "settings")?;

                let v = serde_json::to_value(fields).map_err(serde::ser::Error::custom)?;
                if let Value::Object(obj) = v {
                    for (k, v) in obj {
                        // Skip fields not present
                        map.serialize_entry(&k, &v)?;
                    }
                }
            }
        }

        map.end()
    }
}

impl<'de> Deserialize<'de> for InstanceFields {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>
    {
        let mut obj: BTreeMap<String, Value> = BTreeMap::deserialize(deserializer)?;

        let option_val = obj
            .remove("page")
            .ok_or_else(|| serde::de::Error::custom("missing 'page' field"))?;

        let option_name: String = serde_json::from_value(option_val)
            .map_err(serde::de::Error::custom)?;
        let option_name = option_name.to_lowercase();

        let remainder = Value::Object(obj.into_iter().collect());

        match option_name.as_str() {
            "page" => {
                let fields: OverviewFields = serde_json::from_value(remainder).map_err(serde::de::Error::custom)?;
                Ok(InstanceFields::Overview(fields))
            }
            other => Err(serde::de::Error::custom(format!("unknown option '{}'", other)))
        }
    }
}
