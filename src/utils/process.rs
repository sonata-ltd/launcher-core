//! Conservative process ownership checks shared by filesystem retention.
//! PID reuse and permission/inspection failures keep data rather than delete it.
#[cfg(any(unix, windows))]
use std::io;

#[cfg(unix)]
pub(crate) fn may_be_alive(pid: u32) -> bool {
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return true;
    };
    if pid <= 0 {
        return true;
    }
    // Signal 0 checks existence and permissions without signalling the process.
    let result = unsafe { libc::kill(pid, 0) };
    result == 0 || io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
}

#[cfg(windows)]
pub(crate) fn may_be_alive(pid: u32) -> bool {
    use windows_sys::Win32::{
        Foundation::{CloseHandle, ERROR_INVALID_PARAMETER, STILL_ACTIVE},
        System::Threading::{GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION},
    };
    if pid == 0 {
        return true;
    }
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if handle.is_null() {
        return io::Error::last_os_error().raw_os_error() != Some(ERROR_INVALID_PARAMETER as i32);
    }
    let mut code = 0;
    unsafe {
        let queried = GetExitCodeProcess(handle, &mut code);
        CloseHandle(handle);
        queried == 0 || code == STILL_ACTIVE as u32
    }
}

#[cfg(not(any(unix, windows)))]
pub(crate) fn may_be_alive(_: u32) -> bool {
    true
}

pub(crate) fn read_owner(path: &std::path::Path) -> Option<u32> {
    use std::fs;
    if !fs::symlink_metadata(path).ok()?.is_file() {
        return None;
    }
    read_owner_from(fs::File::open(path).ok()?)
}

pub(crate) fn read_owner_from(file: impl std::io::Read) -> Option<u32> {
    use std::io::Read;
    let mut pid = String::new();
    file.take(11).read_to_string(&mut pid).ok()?;
    if pid.len() > 10 {
        return None;
    }
    pid.parse().ok().filter(|pid| *pid != 0)
}

/// Publish a complete PID without replacing another owner's marker. A numeric
/// prefix of a live PID must never become visible as a different, dead PID.
/// Like other publish helpers, this does not promise power-loss durability.
pub(crate) fn write_owner(path: &std::path::Path) -> std::io::Result<()> {
    use crate::utils::files::directory::Directory;
    let parent = path
        .parent()
        .ok_or_else(|| std::io::Error::other("owner marker has no parent"))?;
    let name = path.file_name().ok_or(std::io::ErrorKind::InvalidInput)?;
    write_owner_in(&Directory::ensure_path(parent)?, name)
}

pub(crate) fn write_owner_in(
    directory: &std::sync::Arc<crate::utils::files::directory::Directory>,
    name: &std::ffi::OsStr,
) -> std::io::Result<()> {
    use std::io::Write;
    directory.with_staged_file(
        |file| {
            file.write_all(std::process::id().to_string().as_bytes())?;
            file.sync_all()
        },
        |staged, ()| directory.publish(staged, directory, name),
    )
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A controllable real child of the current Rust test binary. No shell,
    /// installed JVM, platform-specific sleep command, or user data is needed.
    pub(crate) async fn waiting_child() -> (tempfile::TempDir, async_std::process::Child) {
        let directory = tempfile::tempdir().unwrap();
        let mut child = async_std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "utils::process::tests::controlled_child_fixture",
                "--nocapture",
            ])
            .env("SONATA_TEST_CONTROLLED_CHILD", directory.path())
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let ready = async_std::future::timeout(std::time::Duration::from_secs(10), async {
            while !directory.path().join("ready").is_file() {
                assert!(
                    child.try_status().unwrap().is_none(),
                    "child exited before ready"
                );
                async_std::task::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await;
        if ready.is_err() {
            let _ = child.kill();
            let _ = child.status().await;
            panic!("controlled child did not become ready");
        }
        (directory, child)
    }

    #[test]
    fn controlled_child_fixture() {
        let Some(directory) = std::env::var_os("SONATA_TEST_CONTROLLED_CHILD") else {
            return;
        };
        let directory = std::path::PathBuf::from(directory);
        std::fs::write(directory.join("ready"), b"ready").unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        loop {
            if let Ok(code) = std::fs::read_to_string(directory.join("exit")) {
                std::process::exit(code.parse().unwrap());
            }
            // A failed parent drops its TempDir; do not leave an orphan fixture.
            if !directory.is_dir() || std::time::Instant::now() >= deadline {
                std::process::exit(1);
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    #[test]
    fn owner_publication_is_complete_and_never_replaces_an_existing_marker() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("owner");
        write_owner(&path).unwrap();
        assert_eq!(read_owner(&path), Some(std::process::id()));
        std::fs::write(&path, b"41").unwrap();
        assert_eq!(
            write_owner(&path).unwrap_err().kind(),
            std::io::ErrorKind::AlreadyExists
        );
        assert_eq!(std::fs::read(&path).unwrap(), b"41");
        assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 1);
    }

    #[test]
    fn current_process_and_unknown_ownership_are_never_confirmed_dead() {
        assert!(may_be_alive(std::process::id()));
        assert!(may_be_alive(0));
        let temp = tempfile::tempdir().unwrap();
        let owner = temp.path().join("owner");
        for value in [
            "0",
            "garbage",
            "4294967296",
            "12345678901234567890123456789012345",
        ] {
            std::fs::write(&owner, value).unwrap();
            assert_eq!(read_owner(&owner), None);
        }
    }
}
