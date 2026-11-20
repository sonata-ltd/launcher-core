use std::{
    ffi::OsStr,
    fs, io,
    path::{Path, PathBuf},
    sync::Arc,
};

use crate::utils::{
    files::{directory::Directory, owned_directory::OwnedDirectory},
    process::{may_be_alive as process_may_be_alive, read_owner_from},
};

const OWNER_PID: &str = ".sonata-owner";
const SPAWN_INTENT: &str = ".sonata-starting";
const CHILD_PID: &str = ".sonata-child";
const PREFIX: &str = "run-";

#[derive(Clone)]
pub(crate) struct NativeDirectory {
    directory: Arc<OwnedDirectory>,
    files: Arc<Directory>,
    extracted: PathBuf,
    runtime_pins: Vec<crate::utils::process_pin::ProcessPin>,
}

impl NativeDirectory {
    #[cfg(test)]
    pub(crate) fn create(root: &Path) -> io::Result<Self> {
        Self::create_in(&Directory::ensure_path(root)?)
    }

    /// Natives scratch below a directory the caller already holds open.
    pub(crate) fn create_in(root: &Arc<Directory>) -> io::Result<Self> {
        let directory = OwnedDirectory::create_in(root, PREFIX)?;
        let files = directory
            .directory()
            .create_private_child(OsStr::new("files"))?;
        let extracted = files.path().to_owned();
        Ok(Self {
            directory: Arc::new(directory),
            files,
            extracted,
            runtime_pins: Vec::new(),
        })
    }

    pub(super) fn before_spawn(&self) -> io::Result<()> {
        // This marker precedes process creation. Cleanup conservatively keeps
        // the directory if the launcher dies before it records the game PID.
        self.directory
            .directory()
            .create_private_file(OsStr::new(SPAWN_INTENT))?;
        Ok(())
    }

    pub(super) fn record_child(&self, pid: u32) {
        if let Err(error) = self
            .directory
            .directory()
            .write_bytes(OsStr::new(CHILD_PID), pid.to_string().as_bytes())
        {
            tracing::warn!(%pid, %error, "could not record native directory owner");
        }
    }

    pub(crate) fn finish(self, process_exited: bool) {
        for pin in &self.runtime_pins {
            pin.finish(process_exited);
        }
        if !process_exited {
            self.directory.retain();
            tracing::warn!(path = %self.directory.directory().path().display(), "kept native libraries because process exit was unconfirmed");
        }
    }

    pub(super) fn files(&self) -> &Arc<Directory> {
        &self.files
    }

    pub(crate) fn retain_runtime(&mut self, pin: crate::utils::process_pin::ProcessPin) {
        self.runtime_pins.push(pin);
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
    let root = match instance_root
        .canonicalize()
        .and_then(|root| Directory::open(&root))
    {
        Ok(root) => root,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    for instance in fs::read_dir(root.path())? {
        let instance = instance?;
        if !instance.file_type()?.is_dir() {
            continue;
        }
        let relative = PathBuf::from(instance.file_name()).join(".sonata-natives");
        let Ok(Some(natives)) = root.open_optional(&relative) else {
            continue;
        };
        for directory in fs::read_dir(natives.path())? {
            let directory = directory?;
            if !directory.file_type()?.is_dir()
                || !directory.file_name().to_string_lossy().starts_with(PREFIX)
            {
                continue;
            }
            let path = directory.path();
            let Ok(child) = natives.open_relative(Path::new(&directory.file_name())) else {
                continue;
            };
            if !can_remove(&child, &process_alive) {
                continue;
            }
            if let Err(error) = natives.remove_child_tree(child) {
                if error.kind() != io::ErrorKind::NotFound {
                    tracing::warn!(path = %path.display(), %error, "could not remove stale native directory");
                }
            }
        }
    }
    Ok(())
}

fn can_remove(directory: &Directory, process_alive: &impl Fn(u32) -> bool) -> bool {
    let read_pid = |name: &str| {
        directory
            .open_file(OsStr::new(name))
            .ok()
            .and_then(read_owner_from)
    };
    let Some(owner) = read_pid(OWNER_PID) else {
        return false;
    };
    if process_alive(owner) {
        return false;
    }

    match directory.open_file(OsStr::new(SPAWN_INTENT)) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => true,
        Ok(_) => read_pid(CHILD_PID).is_some_and(|pid| !process_alive(pid)),
        _ => false,
    }
}

pub struct Natives {}

impl Natives {
    pub async fn extract(paths: Vec<PathBuf>, destination: &Path) -> io::Result<()> {
        let destination = destination.to_owned();
        let destination =
            async_std::task::spawn_blocking(move || Directory::ensure_path(&destination)).await?;
        for path in paths {
            Self::extract_archive(path, destination.clone(), vec!["META-INF/".into()]).await?;
        }
        Ok(())
    }

    pub(super) async fn extract_archive(
        path: PathBuf,
        destination: Arc<Directory>,
        excludes: Vec<String>,
    ) -> io::Result<()> {
        async_std::task::spawn_blocking(move || {
            crate::modloader::archive::extract_in(&path, &destination, &excludes)
                .map_err(io::Error::other)
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unconfirmed_exit_retains_scratch_after_the_last_clone_is_dropped() {
        let scope = tempfile::tempdir().unwrap();
        let directory = NativeDirectory::create(scope.path()).unwrap();
        let duplicate = directory.clone();
        let extracted = directory.as_ref().to_owned();
        fs::write(extracted.join("library"), b"native bytes").unwrap();
        directory.finish(false);
        drop(duplicate);

        assert_eq!(
            fs::read(extracted.join("library")).unwrap(),
            b"native bytes"
        );
    }

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
