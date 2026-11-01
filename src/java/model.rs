use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::{instance::model::InstanceId, java::utils::DetectedJavaRuntime};

/// How a runtime got into the registry: added by the user, or found by the
/// search in `java::discovery`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JavaSource {
    #[default]
    Manual,
    Discovered,
}

impl JavaSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Manual => "manual",
            Self::Discovered => "discovered",
        }
    }

    pub(crate) fn parse(s: &str) -> Self {
        match s {
            "discovered" => Self::Discovered,
            _ => Self::Manual,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct JavaRuntime {
    pub id: i64,
    pub version: String,
    pub major: u32,
    pub arch: Option<String>,
    pub exec_path: String,
    pub home_path: String,
    pub vendor: Option<String>,
    pub source: JavaSource,
}

/// New external java runtime structure
#[derive(Debug, Deserialize)]
pub struct NewJavaRuntime {
    pub version: Option<String>,
    pub major: Option<u32>,
    pub arch: Option<String>,
    pub exec_path: Option<PathBuf>,
    pub home_path: Option<PathBuf>,
    pub vendor: Option<String>,
    #[serde(default)]
    pub source: JavaSource,
}

impl From<DetectedJavaRuntime> for NewJavaRuntime {
    fn from(value: DetectedJavaRuntime) -> Self {
        Self {
            version: value.version,
            major: value.major,
            arch: value.arch,
            exec_path: Some(value.exec_path),
            home_path: value.home_path,
            vendor: value.vendor,
            source: JavaSource::Discovered,
        }
    }
}

#[derive(Debug, Clone)]
pub enum JavaEvent {
    ListChanged,
    /// A search finished with `added` new runtimes. `requested` is false for
    /// the first-run search, true when the user asked for it.
    DiscoveryFinished {
        added: usize,
        requested: bool,
    },
    /// The core picked `runtime` for `instance` on its own.
    AutoSelected {
        instance: InstanceId,
        runtime: i64,
    },
    /// No registered runtime fits `instance`, which needs one of `required`
    /// (empty: the version manifest did not say).
    NoSuitableRuntime {
        instance: InstanceId,
        name: String,
        required: Vec<u32>,
    },
}

/// Lets `select::pick` take both `&[JavaRuntime]` and `&[Arc<JavaRuntime>]`.
impl AsRef<JavaRuntime> for JavaRuntime {
    fn as_ref(&self) -> &Self {
        self
    }
}
