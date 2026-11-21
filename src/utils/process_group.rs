//! Process groups for external installer processes.
//!
//! On Unix the processor leads its own process group. Stopping it signals
//! the whole group, and completion is acknowledged only after the group is
//! observed empty, so descendants cannot keep writing after outputs are
//! checked and pins are released. A descendant that leaves the group
//! (`setsid`/`setpgid`) is not tracked. Other hosts keep the previous
//! leader-only behaviour.

use std::{process::Command, time::Duration};

pub(crate) struct ProcessGroup {
    #[cfg(unix)]
    id: libc::pid_t,
    armed: bool,
}

impl ProcessGroup {
    /// Makes the spawned process the leader of a new process group.
    pub(crate) fn isolate(command: &mut Command) {
        #[cfg(unix)]
        std::os::unix::process::CommandExt::process_group(command, 0);
        #[cfg(not(unix))]
        let _ = command;
    }

    pub(crate) fn of_leader(pid: u32) -> Self {
        Self {
            #[cfg(unix)]
            id: pid as libc::pid_t,
            armed: pid != 0,
        }
    }

    /// Sends SIGKILL to every member. Call before the leader is reaped where
    /// possible: an unreaped leader keeps the group id reserved.
    pub(crate) fn kill(&self) {
        #[cfg(unix)]
        if self.armed {
            // SAFETY: plain syscall; a negative id addresses the group.
            unsafe {
                libc::kill(-self.id, libc::SIGKILL);
            }
        }
    }

    #[cfg(unix)]
    fn has_members(&self) -> bool {
        // SAFETY: signal 0 only probes for existence.
        if unsafe { libc::kill(-self.id, 0) } == 0 {
            return true;
        }
        std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
    }

    /// Kills remaining members and waits until the group is empty. Returns
    /// false when members are still observed after `timeout`; the caller
    /// must then treat the process state as unknown.
    pub(crate) async fn terminate(&mut self, timeout: Duration) -> bool {
        #[cfg(unix)]
        {
            if !self.armed {
                return true;
            }
            let started = std::time::Instant::now();
            loop {
                if !self.has_members() {
                    self.armed = false;
                    return true;
                }
                if started.elapsed() > timeout {
                    return false;
                }
                self.kill();
                async_std::task::sleep(Duration::from_millis(10)).await;
            }
        }
        #[cfg(not(unix))]
        {
            let _ = timeout;
            self.armed = false;
            true
        }
    }
}

impl Drop for ProcessGroup {
    fn drop(&mut self) {
        self.kill();
    }
}
