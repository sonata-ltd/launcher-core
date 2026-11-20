//! Durable process retention. Dropping a future never deletes its pin:
//! kill-on-drop is a request, not confirmation that a JVM has stopped.
use super::process;
#[cfg(all(test, unix))]
use std::path::PathBuf;
use std::{fs, io, path::Path};

const DIRECTORY: &str = ".sonata-processes";
const OWNER: &str = ".sonata-owner";
const STARTING: &str = ".sonata-starting";
const CHILD: &str = ".sonata-child";
const EXITED: &str = ".sonata-exited";
const NO_CHILD: &str = ".sonata-no-child";

#[derive(Clone)]
pub(crate) struct ProcessPin {
    directory: std::sync::Arc<super::files::directory::Directory>,
}

impl ProcessPin {
    pub(crate) fn create(resource: &Path) -> io::Result<Self> {
        use std::ffi::OsStr;
        let root = super::files::directory::Directory::ensure_path(resource)?;
        let root = root.create_child(OsStr::new(DIRECTORY))?;
        let directory = root.create_private_child(OsStr::new(&uuid::Uuid::new_v4().to_string()))?;
        process::write_owner_in(&directory, OsStr::new(OWNER))?;
        Ok(Self { directory })
    }

    pub(crate) fn before_spawn(&self) -> io::Result<()> {
        self.marker_new(STARTING, b"1")
    }

    /// Only for a failed preparation that never attempted process creation.
    pub(crate) fn abort_before_spawn(&self) {
        if let Err(error) = self.marker_new(NO_CHILD, b"1") {
            tracing::warn!(%error, "could not acknowledge that no child was created");
        }
    }

    pub(crate) fn record_child(&self, pid: u32) {
        if pid == 0 {
            return;
        }
        if let Err(error) = self.marker_new(CHILD, pid.to_string().as_bytes()) {
            tracing::warn!(%error, pid, "unknown child PID keeps runtime pinned");
        }
    }

    /// Called only after all owned work and a confirmed process exit.
    pub(crate) fn finish(&self, process_exited: bool) {
        if process_exited {
            if let Err(error) = self.marker_new(EXITED, b"1") {
                tracing::warn!(%error, "could not acknowledge process pin completion");
            }
        }
    }

    fn marker_new(&self, name: &str, bytes: &[u8]) -> io::Result<()> {
        use std::{ffi::OsStr, io::Write};
        self.directory.with_staged_file(
            |file| {
                file.write_all(bytes)?;
                file.sync_all()
            },
            |staged, ()| {
                self.directory
                    .publish(staged, &self.directory, OsStr::new(name))
            },
        )
    }
}

pub(crate) fn prepare_spawn(executable: &Path, pins: &[&ProcessPin]) -> io::Result<()> {
    let prepared = check_executable(executable)
        .and_then(|()| pins.iter().try_for_each(|pin| pin.before_spawn()));
    if prepared.is_err() {
        for pin in pins {
            pin.abort_before_spawn();
        }
    }
    prepared
}

fn check_executable(executable: &Path) -> io::Result<()> {
    if !executable.is_absolute() {
        // PATH and cwd resolution belong to the OS process adapter.
        return Ok(());
    }
    let metadata = fs::metadata(executable);
    #[cfg(windows)]
    let metadata = match metadata {
        Err(error)
            if error.kind() == io::ErrorKind::NotFound && executable.extension().is_none() =>
        {
            fs::metadata(executable.with_extension("exe"))
        }
        other => other,
    };
    match metadata {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Err(error),
        Ok(metadata) if metadata.is_dir() => Err(io::Error::new(
            io::ErrorKind::IsADirectory,
            "process executable is a directory",
        )),
        _ => Ok(()),
    }
}

fn marker(path: &Path) -> io::Result<bool> {
    match super::object_store::catalog::read_regular(path, 1) {
        Ok(body) if body == b"1" => Ok(true),
        Ok(_) => Err(io::Error::other("unknown process marker version")),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

fn released(directory: &Path, alive: &impl Fn(u32) -> bool) -> io::Result<bool> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let name = entry.file_name();
        if !entry.file_type()?.is_file()
            || ![OWNER, STARTING, CHILD, EXITED, NO_CHILD]
                .iter()
                .any(|known| name == *known)
        {
            return Ok(false);
        }
    }
    let Some(owner) = process::read_owner(&directory.join(OWNER)) else {
        return Ok(false);
    };
    let starting = marker(&directory.join(STARTING))?;
    let child = match fs::symlink_metadata(directory.join(CHILD)) {
        Ok(_) => {
            let Some(child) = process::read_owner(&directory.join(CHILD)) else {
                return Ok(false);
            };
            if !starting {
                return Ok(false);
            }
            Some(child)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(error),
    };
    let exited = marker(&directory.join(EXITED))?;
    if marker(&directory.join(NO_CHILD))? {
        return Ok(child.is_none() && !exited);
    }
    if exited {
        return Ok(true);
    }
    if alive(owner) {
        return Ok(false);
    }
    if !starting {
        return Ok(true);
    }
    Ok(child.is_some_and(|pid| !alive(pid)))
}

/// Unknown entries, owners, markers, symlinks and reused/live PIDs keep data.
/// Resource ancestors must be validated by its owner under admission.
pub(crate) fn allows_cleanup(resource: &Path) -> io::Result<bool> {
    allows_cleanup_with(resource, &process::may_be_alive)
}

fn allows_cleanup_with(resource: &Path, alive: &impl Fn(u32) -> bool) -> io::Result<bool> {
    let root = resource.join(DIRECTORY);
    match fs::symlink_metadata(&root) {
        Ok(metadata) if metadata.is_dir() => {}
        Ok(_) => return Ok(false),
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(true),
        Err(error) => return Err(error),
    }
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            return Ok(false);
        };
        if !entry.file_type()?.is_dir()
            || uuid::Uuid::parse_str(name).map_or(true, |uuid| uuid.to_string() != name)
            || !released(&entry.path(), alive)?
        {
            return Ok(false);
        }
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completion_not_drop_releases_live_owner_and_unknown_spawn_intent() {
        let root = tempfile::tempdir().unwrap();
        let pin = ProcessPin::create(root.path()).unwrap();
        assert!(!allows_cleanup(root.path()).unwrap());
        pin.before_spawn().unwrap();
        pin.finish(false);
        assert!(!allows_cleanup_with(root.path(), &|_| false).unwrap());
        pin.finish(true);
        assert!(allows_cleanup(root.path()).unwrap());
    }

    #[test]
    fn a_later_marker_failure_releases_earlier_pins_and_retains_unknown_markers() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        let first_pin = ProcessPin::create(first.path()).unwrap();
        let second_pin = ProcessPin::create(second.path()).unwrap();
        fs::write(second_pin.directory.path().join(STARTING), b"unknown").unwrap();
        let executable = std::env::current_exe().unwrap();
        assert!(prepare_spawn(&executable, &[&first_pin, &second_pin]).is_err());
        assert!(allows_cleanup(first.path()).unwrap());
        assert!(!allows_cleanup(second.path()).unwrap_or(false));
        assert_eq!(
            fs::read(second_pin.directory.path().join(STARTING)).unwrap(),
            b"unknown"
        );
    }

    #[test]
    fn child_outlives_owner_and_reused_or_unknown_pids_keep_the_resource() {
        let root = tempfile::tempdir().unwrap();
        let pin = ProcessPin::create(root.path()).unwrap();
        let owner = std::process::id();
        assert!(allows_cleanup_with(root.path(), &|_| false).unwrap());
        pin.before_spawn().unwrap();
        assert!(!allows_cleanup_with(root.path(), &|_| false).unwrap());
        pin.record_child(123);
        assert!(!allows_cleanup_with(root.path(), &|pid| pid == 123).unwrap());
        assert!(!allows_cleanup_with(root.path(), &|pid| pid == owner).unwrap());
        assert!(allows_cleanup_with(root.path(), &|_| false).unwrap());
        drop(pin);
        assert!(!allows_cleanup(root.path()).unwrap());
    }

    #[test]
    fn malformed_markers_and_unknown_entries_do_not_authorize_cleanup() {
        for variant in ["owner", "starting", "exited", "entry"] {
            let root = tempfile::tempdir().unwrap();
            let pin = ProcessPin::create(root.path()).unwrap();
            match variant {
                "owner" => fs::write(pin.directory.path().join(OWNER), b"broken").unwrap(),
                "starting" => fs::write(pin.directory.path().join(STARTING), b"99").unwrap(),
                "exited" => fs::write(pin.directory.path().join(EXITED), b"99").unwrap(),
                _ => fs::write(root.path().join(DIRECTORY).join("unknown"), b"unknown").unwrap(),
            }
            assert!(
                !allows_cleanup_with(root.path(), &|_| false).unwrap_or(false),
                "{variant}"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn pin_or_marker_symlinks_never_release_retention() {
        use std::os::unix::fs::symlink;
        for variant in ["root", "pin", "owner", "starting", "exited", "child"] {
            let root = tempfile::tempdir().unwrap();
            let pin = ProcessPin::create(root.path()).unwrap();
            pin.before_spawn().unwrap();
            pin.record_child(123);
            let path = match variant {
                "root" => root.path().join(DIRECTORY),
                "pin" => pin.directory.path().to_owned(),
                "owner" => pin.directory.path().join(OWNER),
                "starting" => pin.directory.path().join(STARTING),
                "child" => pin.directory.path().join(CHILD),
                _ => {
                    pin.finish(true);
                    pin.directory.path().join(EXITED)
                }
            };
            let outside = root.path().join("outside");
            fs::rename(&path, &outside).unwrap();
            symlink(outside, path).unwrap();
            assert!(
                !allows_cleanup_with(root.path(), &|_| false).unwrap_or(false),
                "{variant}"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn marker_publication_declines_a_replacement_pin_directory() {
        let root = tempfile::tempdir().unwrap();
        let pin = ProcessPin::create(root.path()).unwrap();
        let path = pin.directory.path();
        fs::rename(path, root.path().join("previous")).unwrap();
        fs::create_dir(path).unwrap();
        fs::write(path.join("keep"), b"preserve these bytes").unwrap();

        assert!(pin.before_spawn().is_err());
        assert_eq!(fs::read_dir(path).unwrap().count(), 1);
        assert_eq!(
            fs::read(path.join("keep")).unwrap(),
            b"preserve these bytes"
        );
    }

    #[cfg(unix)]
    #[test]
    fn crashed_owner() {
        let Some(root) = std::env::var_os("SONATA_TEST_PIN_CRASH_ROOT").map(PathBuf::from) else {
            return;
        };
        let pin = ProcessPin::create(&root).unwrap();
        pin.before_spawn().unwrap();
        if std::env::var("SONATA_TEST_PIN_CRASH_PHASE").ok().as_deref() == Some("child") {
            let child = std::process::Command::new("sleep")
                .arg("30")
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .unwrap();
            pin.record_child(child.id());
            fs::write(root.join("child-pid"), child.id().to_string()).unwrap();
        }
        std::process::exit(75);
    }

    #[cfg(unix)]
    #[test]
    fn process_exit_preserves_live_child_and_unknown_spawn_intent() {
        struct Stop(u32);
        impl Drop for Stop {
            fn drop(&mut self) {
                unsafe {
                    libc::kill(self.0 as libc::pid_t, libc::SIGKILL);
                }
            }
        }
        for phase in ["intent", "child"] {
            let root = tempfile::tempdir().unwrap();
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "utils::process_pin::tests::crashed_owner",
                    "--nocapture",
                ])
                .env("SONATA_TEST_PIN_CRASH_ROOT", root.path())
                .env("SONATA_TEST_PIN_CRASH_PHASE", phase)
                .output()
                .unwrap();
            let child = fs::read_to_string(root.path().join("child-pid"))
                .ok()
                .map(|body| Stop(body.parse().unwrap()));
            assert_eq!(
                output.status.code(),
                Some(75),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            if let Some(child) = &child {
                assert!(process::may_be_alive(child.0));
            }
            assert!(!allows_cleanup(root.path()).unwrap(), "{phase}");
            let entries = fs::read_dir(root.path().join(DIRECTORY)).unwrap();
            for entry in entries {
                let path = entry.unwrap().path();
                let owner = process::read_owner(&path.join(OWNER)).unwrap();
                assert!(!process::may_be_alive(owner));
                assert!(!path.join(EXITED).exists());
            }
        }
    }
}
