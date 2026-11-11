use super::*;
use crate::data::config::Config;

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
