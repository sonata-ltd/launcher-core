use async_std::path::PathBuf;

pub enum AvailableLauncherPaths {
    LibsDir,
    AssetsDir,
    InstancesDir,
    InstancesEntryFile,
    HeadersDir,
    MetaDir,
    MetacacheFile,
    LauncherRootDir
}

pub async fn construct_launcher_path<'a>(launcher_root: &PathBuf, path_type: AvailableLauncherPaths) -> PathBuf {
    match path_type {
        AvailableLauncherPaths::LibsDir => {
            return launcher_root.join("libraries");
        },
        AvailableLauncherPaths::AssetsDir => {
            return launcher_root.join("assets");
        },
        AvailableLauncherPaths::InstancesDir => {
            return launcher_root.join("instances")
        }
        AvailableLauncherPaths::InstancesEntryFile => {
            return launcher_root.join("headers/main.json");
        },
        AvailableLauncherPaths::HeadersDir => {
            return launcher_root.join("headers");
        },
        AvailableLauncherPaths::MetaDir => {
            return launcher_root.join("meta");
        },
        AvailableLauncherPaths::MetacacheFile => {
            return launcher_root.join("metacache.json");
        },
        AvailableLauncherPaths::LauncherRootDir => {
            return launcher_root.to_path_buf();
        },
    }
}
