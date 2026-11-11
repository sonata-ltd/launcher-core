use uuid::Uuid;

use crate::{data::layout::LauncherPaths, instance::model::InstanceId, modloader::error::Result};
use std::path::{Path, PathBuf};

#[cfg(test)]
mod tests;

pub(crate) struct InstallWorkspace {
    generation_root: PathBuf,
    relative_maven_dir: PathBuf,
    maven_dir: PathBuf,
    work_dir: PathBuf,
}

impl InstallWorkspace {
    pub async fn create(
        paths: &LauncherPaths,
        instance_id: InstanceId,
    ) -> Result<InstallWorkspace> {
        let generation_uuid = Uuid::new_v4().to_string();

        let generation_root = paths
            .libraries()
            .join("instances")
            .join(instance_id.to_string())
            .join(&generation_uuid);

        let relative_maven_dir = PathBuf::from("instances")
            .join(instance_id.to_string())
            .join(generation_uuid)
            .join("maven");

        let maven_dir = generation_root.join("maven");
        let work_dir = generation_root.join("work");

        let workspace = InstallWorkspace {
            generation_root,
            relative_maven_dir,
            maven_dir,
            work_dir,
        };

        let dir_creation_result = async {
            async_std::fs::create_dir_all(workspace.maven_dir()).await?;
            async_std::fs::create_dir_all(workspace.work_dir()).await?;

            Ok::<(), std::io::Error>(())
        }
        .await;

        if let Err(e) = dir_creation_result {
            if let Err(cleanup_error) = workspace.cleanup_failed().await {
                tracing::warn!(
                    error = %cleanup_error,
                    "Failed to remove partially created workspace"
                );
            }

            return Err(e.into());
        }

        Ok(workspace)
    }

    pub async fn cleanup_failed(&self) -> Result<()> {
        match async_std::fs::remove_dir_all(&self.generation_root).await {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        }
    }

    pub async fn cleanup_work(&self) -> Result<()> {
        match async_std::fs::remove_dir_all(&self.work_dir).await {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        }
    }

    pub fn relative_maven_dir(&self) -> &Path {
        &self.relative_maven_dir
    }

    pub fn maven_dir(&self) -> &Path {
        &self.maven_dir
    }

    pub fn work_dir(&self) -> &Path {
        &self.work_dir
    }
}
