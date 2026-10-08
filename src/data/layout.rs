use std::path::PathBuf;

use async_std::{fs::create_dir_all, io};
use thiserror::Error;

use crate::{
    data::{config::Config, db::DEFAULT_DB_NAME},
    utils::get_home_dir,
};

const LAUNCHER_PREFIX: &'static str = "sonata";

pub struct LauncherPaths {
    root: PathBuf,
    libraries: PathBuf,
    assets: PathBuf,
    instances: PathBuf,
    java_runtimes: PathBuf,
    meta: PathBuf,
    cache_db: PathBuf,
}

#[derive(Debug, Error)]
pub enum LauncherPathsError {
    #[error("Cannot get home dir")]
    HomeNotAvailable,

    #[error("filesystem error occured: {0}")]
    IO(#[from] io::Error),
}

impl LauncherPaths {
    pub async fn resolve(config: &Config) -> Result<Self, LauncherPathsError> {
        let root = config
            .root_override
            .clone()
            .unwrap_or(Self::construct_default_root().await?);

        if let Err(e) = create_dir_all(&root).await {
            return Err(LauncherPathsError::IO(e));
        }

        let d = &config.dirs;

        Ok(Self {
            libraries: d
                .libraries
                .clone()
                .unwrap_or_else(|| root.join("libraries")),
            assets: d.assets.clone().unwrap_or_else(|| root.join("assets")),
            instances: d
                .instances
                .clone()
                .unwrap_or_else(|| root.join("instances")),
            java_runtimes: d.java_runtimes.clone().unwrap_or_else(|| root.join("java")),
            meta: d.meta.clone().unwrap_or_else(|| root.join("meta")),
            cache_db: d
                .cache_db
                .clone()
                .unwrap_or_else(|| root.join(DEFAULT_DB_NAME)),
            root,
        })
    }

    pub fn root(&self) -> &PathBuf {
        &self.root
    }

    pub fn libraries(&self) -> &PathBuf {
        &self.libraries
    }

    pub fn assets(&self) -> &PathBuf {
        &self.assets
    }

    pub fn instances(&self) -> &PathBuf {
        &self.instances
    }

    pub fn java_runtimes(&self) -> &PathBuf {
        &self.java_runtimes
    }

    pub fn meta(&self) -> &PathBuf {
        &self.meta
    }

    pub fn cache_db(&self) -> &PathBuf {
        &self.cache_db
    }

    async fn construct_default_root() -> Result<PathBuf, LauncherPathsError> {
        match get_home_dir().await {
            Some(path) => Ok(path.join(format!(".{}", LAUNCHER_PREFIX))),
            None => Err(LauncherPathsError::HomeNotAvailable),
        }
    }
}
