use std::path::PathBuf;

use crate::data::layout::LauncherPaths;

#[derive(Debug, Clone)]
pub struct InstancePaths {
    root: PathBuf,
    natives: PathBuf,
}

impl InstancePaths {
    pub fn resolve(launcher_paths: &LauncherPaths, dir: &str) -> Self {
        let root = launcher_paths.instances().join(dir);
        let natives = root.join("natives");

        InstancePaths { root, natives }
    }

    pub fn root(&self) -> &PathBuf {
        &self.root
    }
    pub fn natives(&self) -> &PathBuf {
        &self.natives
    }
}
