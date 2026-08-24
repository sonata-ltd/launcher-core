use crate::version::{
    error::{Result, VersionError},
    provider::prism::PrismSource,
};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use std::str::FromStr;
use strum::Display;

pub mod prism;

#[derive(Debug, Clone)]
pub struct VersionSummary {
    pub id: String,
    pub url: String,
    pub kind: VersionKind,
    pub latest_stable: bool,
    pub released_at: Option<DateTime<Utc>>,
    pub provider: MetaProvider,
    pub sha256: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VersionKind {
    Release,
    Snapshot,

    OldSnapshot,
    OldBeta,
    OldAlpha,

    Experiment,

    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq, Display)]
pub enum MetaProvider {
    Mojang,
    Prism,
}

impl MetaProvider {
    pub fn source(&self) -> &'static dyn VersionSource {
        match self {
            MetaProvider::Mojang => todo!(),
            MetaProvider::Prism => &PrismSource,
        }
    }
}

impl FromStr for MetaProvider {
    type Err = VersionError;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "prism" => Ok(MetaProvider::Prism),
            "mojang" => Err(VersionError::ProviderUnsupported(s.to_string())),
            _ => Err(VersionError::UnknownProvider(s.to_string())),
        }
    }
}

pub trait VersionSource: Send + Sync {
    fn id(&self) -> MetaProvider;
    fn base_url(&self) -> &'static str;
    fn index_url(&self) -> String;
    fn parse_index(&self, raw: &[u8]) -> Result<Vec<VersionSummary>>;
}
