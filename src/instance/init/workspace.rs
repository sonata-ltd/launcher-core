use uuid::Uuid;

use crate::{
    data::layout::LauncherPaths,
    instance::init::error::Result,
    instance::model::InstanceId,
    utils::files::{directory::Directory, identity::FileIdentity},
};
use std::{
    ffi::OsStr,
    path::{Path, PathBuf},
    sync::Arc,
};

pub(crate) struct InstallWorkspace {
    generation_root: PathBuf,
    relative_maven_dir: PathBuf,
    maven_dir: PathBuf,
    work_dir: PathBuf,
    generation_identity: FileIdentity,
    _admission: crate::utils::object_store::StoreAccess,
}

impl InstallWorkspace {
    #[cfg(test)]
    pub async fn create(
        paths: &LauncherPaths,
        instance_id: InstanceId,
    ) -> Result<InstallWorkspace> {
        Self::create_with_cancellation(
            paths,
            instance_id,
            &crate::utils::cancellation::Cancellation::new(),
        )
        .await
    }

    #[cfg(test)]
    pub async fn create_with_cancellation(
        paths: &LauncherPaths,
        instance_id: InstanceId,
        cancel: &crate::utils::cancellation::Cancellation,
    ) -> Result<InstallWorkspace> {
        Self::create_inner(paths, instance_id, cancel, None).await
    }

    pub async fn create_owned(
        paths: &LauncherPaths,
        instance_id: InstanceId,
        cancel: &crate::utils::cancellation::Cancellation,
        db: &crate::data::db::Database,
    ) -> Result<InstallWorkspace> {
        Self::create_inner(paths, instance_id, cancel, Some(db)).await
    }

    async fn create_inner(
        paths: &LauncherPaths,
        instance_id: InstanceId,
        cancel: &crate::utils::cancellation::Cancellation,
        db: Option<&crate::data::db::Database>,
    ) -> Result<InstallWorkspace> {
        let store = crate::utils::object_store::ObjectStore::new(paths.libraries().clone());
        let admission = store.access(cancel).await?;
        let db_owner = if let Some(db) = db {
            let owner = db
                .retention_owner()
                .await
                .map_err(crate::data::db::DbError::from)?;
            if !crate::utils::object_store::catalog::RootCatalog::covers(
                admission.root(),
                owner.as_ref(),
            )? {
                return Err(std::io::Error::other(
                    "uncatalogued runtime writer cannot create a generation",
                )
                .into());
            }
            owner
        } else {
            None
        };
        let generation = Uuid::new_v4();
        let generation_uuid = generation.to_string();
        let owner = super::generation_gc::GenerationOwner::new(
            db_owner.map(|owner| owner.identity),
            instance_id.get(),
            generation,
        )?;

        let instance_dir = PathBuf::from("instances").join(instance_id.to_string());
        let generation_relative = instance_dir.join(&generation_uuid);
        let generation_root = paths.libraries().join(&generation_relative);
        let relative_maven_dir = generation_relative.join("maven");

        let maven_dir = generation_root.join("maven");
        let work_dir = generation_root.join("work");

        cancel.check()?;
        let parent = admission.directory().create_relative(&instance_dir)?;
        // An existing UUID directory is never ours, even if its PID marker matches.
        let generation_directory = parent.create_private_child(OsStr::new(&generation_uuid))?;
        let workspace = InstallWorkspace {
            generation_root,
            relative_maven_dir,
            maven_dir,
            work_dir,
            generation_identity: generation_directory.identity()?,
            _admission: admission,
        };

        let dir_creation_result = (|| {
            // Ownership is visible before any useful runtime outputs.
            generation_directory.verify_location()?;
            crate::utils::process::write_owner_in(
                &generation_directory,
                OsStr::new(".sonata-owner"),
            )?;
            owner.write_in(&generation_directory)?;
            generation_directory.create_private_child(OsStr::new("maven"))?;
            generation_directory.create_private_child(OsStr::new("work"))?;

            Ok::<(), std::io::Error>(())
        })();
        drop(generation_directory);

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
        if !self.cleanup_allowed()? {
            return Ok(());
        }
        let result = self.open_generation();
        let result = match result {
            Ok((parent, generation)) => {
                async_std::task::spawn_blocking(move || parent.remove_child_tree(generation)).await
            }
            Err(error) => Err(error),
        };
        match result {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        }
    }

    pub async fn cleanup_work(&self) -> Result<()> {
        if !self.cleanup_allowed()? {
            return Ok(());
        }
        match std::fs::symlink_metadata(&self.work_dir) {
            Ok(metadata) if metadata.is_dir() => {}
            Ok(_) => return Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error.into()),
        }
        let result = self.open_generation().and_then(|(_, generation)| {
            let work = generation.open_relative(Path::new("work"))?;
            Ok((generation, work))
        });
        let result = match result {
            Ok((generation, work)) => {
                async_std::task::spawn_blocking(move || generation.remove_child_tree(work)).await
            }
            Err(error) => Err(error),
        };
        match result {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        }
    }

    pub fn relative_maven_dir(&self) -> &Path {
        &self.relative_maven_dir
    }

    fn cleanup_allowed(&self) -> Result<bool> {
        match std::fs::symlink_metadata(&self.generation_root) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(true),
            Err(error) => return Err(error.into()),
            Ok(metadata) if metadata.is_dir() => {}
            Ok(_) => return Ok(false),
        }
        let owned = crate::utils::process::read_owner(&self.generation_root.join(".sonata-owner"))
            == Some(std::process::id());
        let processes_finished = crate::utils::process_pin::allows_cleanup(&self.generation_root)?;
        if !owned || !processes_finished {
            tracing::debug!(root = %self.generation_root.display(), "kept workspace with live or unknown process ownership");
        }
        let same_generation = FileIdentity::at(&self.generation_root)? == self.generation_identity;
        Ok(owned && processes_finished && same_generation)
    }

    pub fn maven_dir(&self) -> &Path {
        &self.maven_dir
    }

    pub(crate) fn open_work_dir(&self) -> Result<Arc<Directory>> {
        let (_, generation) = self.open_generation()?;
        Ok(generation.open_relative(Path::new("work"))?)
    }

    pub(crate) fn open_maven_dir(&self) -> Result<Arc<Directory>> {
        let (_, generation) = self.open_generation()?;
        Ok(generation.open_relative(Path::new("maven"))?)
    }

    fn open_generation(&self) -> std::io::Result<(Arc<Directory>, Arc<Directory>)> {
        let generation = self
            .relative_maven_dir
            .parent()
            .ok_or(std::io::ErrorKind::InvalidInput)
            .map_err(std::io::Error::from)?;
        let root = self._admission.directory();
        root.verify_location()?;
        let relative_parent = generation
            .parent()
            .ok_or(std::io::ErrorKind::InvalidInput)?;
        let name = generation
            .file_name()
            .ok_or(std::io::ErrorKind::InvalidInput)?;
        let parent = root.open_relative(relative_parent)?;
        let generation = parent.open_relative(Path::new(name))?;
        if generation.identity()? != self.generation_identity {
            return Err(std::io::Error::other("installation workspace changed"));
        }
        Ok((parent, generation))
    }

    #[cfg(test)]
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

    #[async_std::test]
    async fn cleanup_keeps_live_or_unknown_installers_until_confirmed_exit() {
        let temp = tempfile::tempdir().unwrap();
        let paths = paths(temp.path()).await;
        let workspace = InstallWorkspace::create(&paths, 42.into()).await.unwrap();
        let pin =
            crate::utils::process_pin::ProcessPin::create(&workspace.generation_root).unwrap();
        pin.before_spawn().unwrap();
        pin.record_child(std::process::id());
        std::fs::write(workspace.work_dir().join("installer.log"), b"in use").unwrap();
        workspace.cleanup_work().await.unwrap();
        workspace.cleanup_failed().await.unwrap();
        assert!(workspace.work_dir().join("installer.log").is_file());
        pin.finish(false);
        workspace.cleanup_failed().await.unwrap();
        assert!(workspace.maven_dir().is_dir());
        pin.finish(true);
        workspace.cleanup_work().await.unwrap();
        assert!(!workspace.work_dir().exists());
        workspace.cleanup_failed().await.unwrap();
        assert!(!workspace.generation_root.exists());
    }

    #[async_std::test]
    async fn workspace_ownership_and_shared_admission_precede_outputs() {
        let temp = tempfile::tempdir().unwrap();
        let paths = paths(temp.path()).await;
        let workspace = InstallWorkspace::create(&paths, 42.into()).await.unwrap();
        assert_eq!(
            crate::utils::process::read_owner(&workspace.generation_root.join(".sonata-owner")),
            Some(std::process::id())
        );
        let store = crate::utils::object_store::ObjectStore::new(paths.libraries().clone());
        assert!(store.try_maintenance().is_err());
        drop(workspace);
        assert!(store.try_maintenance().is_ok());
    }

    #[async_std::test]
    async fn cancelling_workspace_admission_creates_no_generation() {
        let temp = tempfile::tempdir().unwrap();
        let paths = paths(temp.path()).await;
        let store = crate::utils::object_store::ObjectStore::new(paths.libraries().clone());
        let _maintenance = store.try_maintenance().unwrap();
        let cancel = crate::utils::cancellation::Cancellation::new();
        cancel.cancel();
        assert!(
            InstallWorkspace::create_with_cancellation(&paths, 42.into(), &cancel)
                .await
                .is_err()
        );
        assert!(!paths.libraries().join("instances").exists());
    }

    #[cfg(unix)]
    #[async_std::test]
    async fn escaped_generations_and_symlink_process_pins_are_not_cleaned() {
        use std::os::unix::fs::symlink;
        let temp = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let paths = paths(temp.path()).await;
        std::fs::create_dir_all(paths.libraries()).unwrap();
        symlink(outside.path(), paths.libraries().join("instances")).unwrap();
        assert!(InstallWorkspace::create(&paths, 42.into()).await.is_err());
        std::fs::remove_file(paths.libraries().join("instances")).unwrap();
        let workspace = InstallWorkspace::create(&paths, 42.into()).await.unwrap();
        symlink(
            outside.path(),
            workspace.generation_root.join(".sonata-processes"),
        )
        .unwrap();
        workspace.cleanup_failed().await.unwrap();
        assert!(workspace.maven_dir().is_dir());
    }
}
