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
    use std::{fs, io::Read};
    if !fs::symlink_metadata(path).ok()?.is_file() {
        return None;
    }
    let mut pid = String::new();
    fs::File::open(path)
        .ok()?
        .take(11)
        .read_to_string(&mut pid)
        .ok()?;
    if pid.len() > 10 {
        return None;
    }
    pid.parse().ok().filter(|pid| *pid != 0)
}

#[cfg(test)]
mod tests {
    use super::*;
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
