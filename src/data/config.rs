use std::path::PathBuf;

use thiserror::Error;

use crate::utils::get_home_dir;

pub struct Config {
    db_path: PathBuf,
    launcher_root_path: PathBuf
}

const DEFAULT_DB_NAME: &'static str = "cache.db";

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("Cannot get home dir")]
    HomeNotAvailable
}

impl Config {
    pub async fn init() -> Result<Self, ConfigError> {
        let root_path = match get_home_dir().await {
            Some(path) => path,
            None => return Err(ConfigError::HomeNotAvailable)
        };

        let db_path = root_path.join(DEFAULT_DB_NAME);
        let launcher_root_path = root_path.join(".sonata");

        Ok(Config { db_path, launcher_root_path })
    }

    pub fn get_db_path(&self) -> &PathBuf {
        &self.db_path
    }

    pub fn take_launcher_root_path(self) -> PathBuf {
        self.launcher_root_path
    }
}
