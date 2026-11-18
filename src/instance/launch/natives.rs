use std::{
    fs, io,
    path::{Path, PathBuf},
    sync::Arc,
};

use crate::utils::process::{may_be_alive as process_may_be_alive, read_owner as read_pid};

const OWNER_PID: &str = ".sonata-owner";
const SPAWN_INTENT: &str = ".sonata-starting";
const CHILD_PID: &str = ".sonata-child";
const PREFIX: &str = "run-";

#[derive(Clone)]
pub(crate) struct NativeDirectory {
    directory: Arc<tempfile::TempDir>,
    extracted: PathBuf,
    runtime_pin: Option<crate::utils::process_pin::ProcessPin>,
}

impl NativeDirectory {
    pub(crate) fn create(root: &Path) -> io::Result<Self> {
        fs::create_dir_all(root)?;
        if !fs::symlink_metadata(root)?.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "natives root is not a directory",
            ));
        }
        let directory = tempfile::Builder::new().prefix(PREFIX).tempdir_in(root)?;
        let extracted = directory.path().join("files");
        fs::create_dir(&extracted)?;
        crate::utils::files::atomic_write(
            &directory.path().join(OWNER_PID),
            std::process::id().to_string().as_bytes(),
        )
        .map_err(io::Error::other)?;
        Ok(Self {
            directory: Arc::new(directory),
            extracted,
            runtime_pin: None,
        })
    }

    pub(super) fn before_spawn(&self) -> io::Result<()> {
        // This marker precedes process creation. Cleanup conservatively keeps
        // the directory if the launcher dies before it records the game PID.
        fs::File::create_new(self.directory.path().join(SPAWN_INTENT))?;
        Ok(())
    }

    pub(super) fn record_child(&self, pid: u32) {
        if let Err(error) = crate::utils::files::atomic_write(
            &self.directory.path().join(CHILD_PID),
            pid.to_string().as_bytes(),
        ) {
            tracing::warn!(%pid, %error, "could not record native directory owner");
        }
    }

    pub(crate) fn finish(self, process_exited: bool) {
        if let Some(pin) = &self.runtime_pin {
            pin.finish(process_exited);
        }
        if let Ok(directory) = Arc::try_unwrap(self.directory) {
            if process_exited {
                let path = directory.path().to_owned();
                if let Err(error) = directory.close() {
                    tracing::warn!(path = %path.display(), %error, "could not remove native libraries");
                }
            } else {
                let path = directory.keep();
                tracing::warn!(path = %path.display(), "kept native libraries because process exit was unconfirmed");
            }
        }
    }

    pub(crate) fn retain_runtime(&mut self, pin: crate::utils::process_pin::ProcessPin) {
        self.runtime_pin = Some(pin);
    }
}

impl AsRef<Path> for NativeDirectory {
    fn as_ref(&self) -> &Path {
        &self.extracted
    }
}

pub(crate) async fn sweep_stale(instance_root: PathBuf) {
    async_std::task::spawn_blocking(move || {
        if let Err(error) = sweep(&instance_root, process_may_be_alive) {
            tracing::warn!(path = %instance_root.display(), %error, "could not sweep stale native directories");
        }
    })
    .await;
}

fn sweep(instance_root: &Path, process_alive: impl Fn(u32) -> bool) -> io::Result<()> {
    let instances = match fs::read_dir(instance_root) {
        Ok(instances) => instances,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    for instance in instances {
        let instance = instance?;
        if !instance.file_type()?.is_dir() {
            continue;
        }
        let natives = instance.path().join(".sonata-natives");
        if !fs::symlink_metadata(&natives).is_ok_and(|metadata| metadata.is_dir()) {
            continue;
        }
        for directory in fs::read_dir(natives)? {
            let directory = directory?;
            if !directory.file_type()?.is_dir()
                || !directory.file_name().to_string_lossy().starts_with(PREFIX)
            {
                continue;
            }
            let path = directory.path();
            if !can_remove(&path, &process_alive) {
                continue;
            }
            if let Err(error) = fs::remove_dir_all(&path) {
                if error.kind() != io::ErrorKind::NotFound {
                    tracing::warn!(path = %path.display(), %error, "could not remove stale native directory");
                }
            }
        }
    }
    Ok(())
}

fn can_remove(directory: &Path, process_alive: &impl Fn(u32) -> bool) -> bool {
    let Some(owner) = read_pid(&directory.join(OWNER_PID)) else {
        return false;
    };
    if process_alive(owner) {
        return false;
    }

    match fs::symlink_metadata(directory.join(SPAWN_INTENT)) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => true,
        Ok(marker) if marker.is_file() => {
            read_pid(&directory.join(CHILD_PID)).is_some_and(|pid| !process_alive(pid))
        }
        _ => false,
    }
}

pub struct Natives {}

impl Natives {
    pub async fn extract(paths: Vec<PathBuf>, destination: &Path) -> io::Result<()> {
        async_std::fs::create_dir_all(destination).await?;
        for path in paths {
            Self::extract_archive(path, destination.to_owned(), vec!["META-INF/".into()]).await?;
        }
        Ok(())
    }

    pub(super) async fn extract_archive(
        path: PathBuf,
        destination: impl AsRef<Path> + Send + 'static,
        excludes: Vec<String>,
    ) -> io::Result<()> {
        async_std::task::spawn_blocking(move || {
            crate::modloader::archive::extract(&path, destination.as_ref(), &excludes)
                .map_err(io::Error::other)
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stale_sweep_keeps_live_launches_and_unverified_ownership() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("instance/.sonata-natives");
        fs::create_dir_all(&root).unwrap();
        let prepare = |name, owner: Option<u32>, starting: bool, child: Option<u32>| {
            let path = root.join(name);
            fs::create_dir(&path).unwrap();
            if let Some(owner) = owner {
                fs::write(path.join(OWNER_PID), owner.to_string()).unwrap();
            }
            if starting {
                fs::File::create_new(path.join(SPAWN_INTENT)).unwrap();
            }
            if let Some(child) = child {
                fs::write(path.join(CHILD_PID), child.to_string()).unwrap();
            }
            path
        };
        let preparing = prepare("run-preparing", Some(104), false, None);
        let active = prepare("run-active", Some(101), true, Some(105));
        let unknown = prepare("run-unknown", Some(101), true, None);
        let untracked = prepare("legacy-uuid", None, false, None);
        let exited = prepare("run-exited", Some(101), true, Some(103));
        let abandoned = prepare("run-abandoned", Some(101), false, None);

        sweep(temporary.path(), |pid| matches!(pid, 104 | 105)).unwrap();
        for path in [preparing, active, unknown, untracked] {
            assert!(path.exists());
        }
        for path in [exited, abandoned] {
            assert!(!path.exists());
        }
    }

    #[async_std::test]
    async fn corrupt_native_archive_is_returned_to_the_caller() {
        let temp = tempfile::tempdir().unwrap();
        let archive = temp.path().join("natives.jar");
        std::fs::write(&archive, b"not a ZIP archive").unwrap();
        let destination = temp.path().join("extracted");
        let error = Natives::extract(vec![archive], &destination)
            .await
            .unwrap_err();
        assert!(error.get_ref().is_some());
        assert_eq!(std::fs::read_dir(destination).unwrap().count(), 0);
    }
}
