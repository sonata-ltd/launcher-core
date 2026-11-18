use serde::Deserialize;
use std::collections::HashMap;
use thiserror::Error;

pub mod args;
pub mod arguments;
pub(crate) mod natives;
pub(crate) mod prepare;
pub(crate) mod resolved;
pub mod traits;

#[derive(Debug, Error)]
pub enum LaunchError {
    #[error(transparent)]
    Arguments(#[from] arguments::ArgumentError),

    #[error("classpath contains the platform path separator: {0}")]
    ClasspathSeparator(String),

    #[error("classpath is not valid unicode")]
    ClasspathNotUnicode,

    #[error("no java runtime selected")]
    JavaRuntimeMissing,

    #[error("Java {actual} is incompatible with this profile (supported majors: {supported:?}, allow newer: {allow_newer})")]
    IncompatibleJavaVersion {
        actual: u32,
        supported: Vec<u32>,
        allow_newer: bool,
    },

    #[error("Java architecture {actual} is incompatible with host architecture {host}")]
    IncompatibleJavaArchitecture { actual: String, host: &'static str },

    #[error("no version manifest provided")]
    ManifestMissing,

    #[error("failed to spawn java process: {0}")]
    Spawn(#[source] std::io::Error),

    #[error("failed to extract natives: {0}")]
    Natives(#[source] std::io::Error),

    #[error("could not retain runtime libraries: {0}")]
    Retention(#[source] std::io::Error),
}

#[derive(Deserialize, Debug)]
pub struct ClientOptions {
    pub classpath: Vec<String>,
    pub main_class: String,
    pub game_args: HashMap<String, String>,
}
