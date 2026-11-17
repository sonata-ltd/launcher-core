use std::path::PathBuf;

use crate::version::provider::MetaProvider;

pub struct Config {
    pub root_override: Option<PathBuf>,
    pub dirs: DirOverrides,
    pub database_url: Option<String>,

    pub meta_provider: MetaProvider,
}

#[derive(Default)]
pub struct DirOverrides {
    pub libraries: Option<PathBuf>,
    pub assets: Option<PathBuf>,
    pub instances: Option<PathBuf>,
    pub java_runtimes: Option<PathBuf>,
    pub meta: Option<PathBuf>,
    pub cache_db: Option<PathBuf>,
}

pub const DEFAULT_EVENTBUS_CAPACITY: usize = 1024;

impl Config {
    pub fn init() -> Self {
        Self {
            root_override: None,
            dirs: DirOverrides::default(),
            database_url: None,

            meta_provider: MetaProvider::Prism,
        }
    }
}
