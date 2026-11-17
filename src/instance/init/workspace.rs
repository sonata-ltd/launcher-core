use uuid::Uuid;

use crate::{
    data::layout::LauncherPaths, instance::init::error::Result, instance::model::InstanceId,
};
use std::path::{Path, PathBuf};

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

#[cfg(test)]
mod tests {
    use std::path::Path;

    use crate::{data::layout::LauncherPaths, instance::init::workspace::InstallWorkspace, Config};

    async fn paths(root: &Path) -> LauncherPaths {
        LauncherPaths::resolve(&Config {
            root_override: Some(root.to_path_buf()),
            ..Config::init()
        })
        .await
        .unwrap()
    }

    #[async_std::test]
    async fn creates_distinct_generations_with_portable_library_paths() {
        let temp = tempfile::tempdir().unwrap();
        let paths = paths(temp.path()).await;
        let first = InstallWorkspace::create(&paths, 42.into()).await.unwrap();
        let second = InstallWorkspace::create(&paths, 42.into()).await.unwrap();

        assert_ne!(first.generation_root, second.generation_root);
        for workspace in [&first, &second] {
            assert!(workspace.maven_dir().is_dir());
            assert!(workspace.work_dir().is_dir());
            assert!(workspace
                .generation_root
                .starts_with(paths.libraries().join("instances/42")));
            assert!(!workspace.relative_maven_dir().is_absolute());
            assert_eq!(
                paths.libraries().join(workspace.relative_maven_dir()),
                workspace.maven_dir()
            );
        }
    }

    #[async_std::test]
    async fn cleanup_work_preserves_runtime_files_and_other_generations() {
        let temp = tempfile::tempdir().unwrap();
        let paths = paths(temp.path()).await;
        let previous = InstallWorkspace::create(&paths, 42.into()).await.unwrap();
        let workspace = InstallWorkspace::create(&paths, 42.into()).await.unwrap();
        let runtime = workspace.maven_dir().join("client.jar");
        std::fs::write(&runtime, b"runtime").unwrap();
        std::fs::create_dir_all(workspace.work_dir().join("installer")).unwrap();
        std::fs::write(workspace.work_dir().join("installer/log.txt"), b"log").unwrap();

        workspace.cleanup_work().await.unwrap();

        assert!(!workspace.work_dir().exists());
        assert_eq!(std::fs::read(runtime).unwrap(), b"runtime");
        assert!(previous.maven_dir().is_dir());
        assert!(previous.work_dir().is_dir());
    }

    #[async_std::test]
    async fn cleanup_failed_removes_entire_generation_and_preserves_previous_files() {
        let temp = tempfile::tempdir().unwrap();
        let paths = paths(temp.path()).await;
        let previous = InstallWorkspace::create(&paths, 42.into()).await.unwrap();
        let previous_runtime = previous.maven_dir().join("client.jar");
        std::fs::write(&previous_runtime, b"previous runtime").unwrap();
        let workspace = InstallWorkspace::create(&paths, 42.into()).await.unwrap();
        std::fs::write(workspace.maven_dir().join("partial.jar"), b"partial").unwrap();
        std::fs::write(workspace.work_dir().join("installer.jar"), b"installer").unwrap();
        std::fs::write(workspace.generation_root.join("extra.log"), b"log").unwrap();

        workspace.cleanup_failed().await.unwrap();

        assert!(!workspace.generation_root.exists());
        assert_eq!(
            std::fs::read(previous_runtime).unwrap(),
            b"previous runtime"
        );
        assert!(previous.work_dir().is_dir());
    }

    #[async_std::test]
    async fn cleanup_is_successful_when_directories_are_already_absent() {
        let temp = tempfile::tempdir().unwrap();
        let paths = paths(temp.path()).await;
        let workspace = InstallWorkspace::create(&paths, 42.into()).await.unwrap();

        workspace.cleanup_work().await.unwrap();
        workspace.cleanup_work().await.unwrap();
        workspace.cleanup_failed().await.unwrap();
        workspace.cleanup_failed().await.unwrap();
        workspace.cleanup_work().await.unwrap();

        assert!(!workspace.generation_root.exists());
    }
}
