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
        Self::resolve_with_default(config, Self::construct_default_root).await
    }

    async fn resolve_with_default<F, Fut>(
        config: &Config,
        default: F,
    ) -> Result<Self, LauncherPathsError>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<PathBuf, LauncherPathsError>>,
    {
        let root = match &config.root_override {
            Some(root) => root.clone(),
            None => default().await?,
        };

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

#[cfg(test)]
mod tests {
    use super::*;

    #[async_std::test]
    async fn explicit_root_never_evaluates_unavailable_default_home() {
        let directory = tempfile::tempdir().unwrap();
        let config = Config {
            root_override: Some(directory.path().into()),
            ..Config::init()
        };
        let paths = LauncherPaths::resolve_with_default(&config, || async {
            panic!("default home must be lazy")
        })
        .await
        .unwrap();
        assert_eq!(paths.root(), directory.path());
        assert!(matches!(
            LauncherPaths::resolve_with_default(&Config::init(), || async {
                Err(LauncherPathsError::HomeNotAvailable)
            })
            .await,
            Err(LauncherPathsError::HomeNotAvailable)
        ));
    }
}
