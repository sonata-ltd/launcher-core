use std::path::{Path, PathBuf};

use getset::Getters;
use serde::Deserialize;

#[derive(Debug, Deserialize, Clone, Getters)]
#[get = "pub"]
pub struct InstancePaths {
    root: PathBuf,
    libs: PathBuf,
    assets: PathBuf,
    instance: PathBuf,
    instance_manifest_file: PathBuf,
    instances_list_file: PathBuf,
    headers: PathBuf,
    meta: PathBuf,
    version_manifest_file: Option<PathBuf>,
    metacache_file: PathBuf,
}

impl InstancePaths {
    // Return Libs path, Assets path, Instances path
    pub fn get_required_paths(instance_name: &str, launcher_root: &PathBuf) -> Self {
        InstancePaths {
            libs: launcher_root.join("libraries"),
            assets: launcher_root.join("assets"),
            instance: launcher_root.join("instances").join(instance_name),
            instance_manifest_file: launcher_root.join("headers").join(instance_name),
            instances_list_file: launcher_root.join("headers").join("main.json"),
            headers: launcher_root.join("headers"),
            meta: launcher_root.join("meta"),
            version_manifest_file: None,
            metacache_file: launcher_root.join("metacache.json"),
            root: launcher_root.clone(),
        }
    }

    pub fn set_version_manifest_file<P: AsRef<Path>>(&mut self, path: P) -> &mut Self {
        self.version_manifest_file = Some(path.as_ref().into());
        self
    }
}
