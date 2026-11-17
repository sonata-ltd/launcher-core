use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Debug, Deserialize)]
pub struct ModernInstallProfile {
    #[serde(default)]
    pub spec: Option<u32>,
    pub minecraft: String,
    pub json: String,
    #[serde(default)]
    pub libraries: Vec<Value>,
    #[serde(default)]
    pub data: BTreeMap<String, SideData>,
    #[serde(default)]
    pub processors: Vec<Processor>,
}

#[derive(Debug, Deserialize)]
pub struct SideData {
    pub client: Option<String>,
    pub server: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct Processor {
    pub sides: Option<Vec<String>>,
    pub jar: String,
    #[serde(default)]
    pub classpath: Vec<String>,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub outputs: BTreeMap<String, String>,
}

impl Processor {
    pub fn applies_to_client(&self) -> bool {
        self.sides
            .as_ref()
            .is_none_or(|sides| sides.iter().any(|s| s == "client"))
    }
}
