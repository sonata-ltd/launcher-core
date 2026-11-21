//! OS confinement for external installer processes.
//!
//! On Linux, Landlock limits every write of the process and its descendants
//! to one held directory (bound by descriptor, not by path) plus `/dev/null`,
//! and scopes signals and abstract Unix sockets to the confined tree where
//! the kernel supports it. Reads, execution and network are not restricted.
//! Without Landlock ABI 3 the confinement is refused, never skipped. Other
//! operating systems have no confinement yet; callers document that limit.

use std::{io, process::Command};

use super::files::directory::Directory;

/// A prepared confinement; keep it alive until the process is spawned.
pub(crate) struct Confinement {
    #[cfg(target_os = "linux")]
    ruleset: std::os::fd::OwnedFd,
}

impl Confinement {
    /// Allows writes only beneath `writable`.
    #[cfg(target_os = "linux")]
    pub(crate) fn writable_beneath(writable: &Directory) -> io::Result<Self> {
        linux::ruleset(writable).map(|ruleset| Self { ruleset })
    }

    #[cfg(not(target_os = "linux"))]
    pub(crate) fn writable_beneath(_writable: &Directory) -> io::Result<Self> {
        Ok(Self {})
    }

    /// Applies the confinement in the child between fork and exec.
    #[cfg(target_os = "linux")]
    pub(crate) fn apply(&self, command: &mut Command) {
        use std::os::{fd::AsRawFd, unix::process::CommandExt};
        let ruleset = self.ruleset.as_raw_fd();
        // SAFETY: the closure only performs two async-signal-safe syscalls on
        // a descriptor that stays open until spawn returns.
        unsafe {
            command.pre_exec(move || linux::restrict_self(ruleset));
        }
    }

    #[cfg(not(target_os = "linux"))]
    pub(crate) fn apply(&self, _command: &mut Command) {}
}

/// Whether `error` reports that this host cannot confine processes.
pub(crate) fn is_unavailable(error: &io::Error) -> bool {
    error.kind() == io::ErrorKind::Unsupported
}

#[cfg(target_os = "linux")]
mod linux {
    use std::{
        fs::File,
        io,
        os::{
            fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, OwnedFd, RawFd},
            unix::fs::OpenOptionsExt,
        },
    };

    use super::Directory;

    const CREATE_RULESET_VERSION: libc::c_uint = 1;
    const RULE_PATH_BENEATH: libc::c_uint = 1;
    pub(super) const MINIMUM_ABI: i64 = 3;
    pub(super) const SCOPE_ABI: i64 = 6;

    const WRITE_FILE: u64 = 1 << 1;
    const TRUNCATE: u64 = 1 << 14;
    /// Every right that creates, changes, links, renames or removes a file.
    pub(super) const WRITE_ACCESS: u64 = WRITE_FILE
        | 1 << 4 // REMOVE_DIR
        | 1 << 5 // REMOVE_FILE
        | 1 << 6 // MAKE_CHAR
        | 1 << 7 // MAKE_DIR
        | 1 << 8 // MAKE_REG
        | 1 << 9 // MAKE_SOCK
        | 1 << 10 // MAKE_FIFO
        | 1 << 11 // MAKE_BLOCK
        | 1 << 12 // MAKE_SYM
        | 1 << 13 // REFER
        | TRUNCATE;
    const SCOPES: u64 = 1 << 0 /* ABSTRACT_UNIX_SOCKET */ | 1 << 1 /* SIGNAL */;

    #[repr(C)]
    struct RulesetAttr {
        handled_access_fs: u64,
        handled_access_net: u64,
        scoped: u64,
    }

    #[repr(C, packed)]
    struct PathBeneathAttr {
        allowed_access: u64,
        parent_fd: i32,
    }

    fn unavailable(detail: String) -> io::Error {
        io::Error::new(
            io::ErrorKind::Unsupported,
            format!("process confinement requires Landlock ABI {MINIMUM_ABI} or later: {detail}"),
        )
    }

    pub(super) fn required(abi: io::Result<i64>) -> io::Result<i64> {
        match abi {
            Ok(abi) if abi >= MINIMUM_ABI => Ok(abi),
            Ok(abi) => Err(unavailable(format!("kernel reports ABI {abi}"))),
            Err(error) => Err(unavailable(error.to_string())),
        }
    }

    pub(super) fn abi() -> io::Result<i64> {
        // SAFETY: the version query takes no attribute pointer.
        let result = unsafe {
            libc::syscall(
                libc::SYS_landlock_create_ruleset,
                std::ptr::null::<RulesetAttr>(),
                0usize,
                CREATE_RULESET_VERSION,
            )
        };
        if result < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(result)
        }
    }

    fn add(ruleset: &OwnedFd, parent: BorrowedFd<'_>, allowed_access: u64) -> io::Result<()> {
        let rule = PathBeneathAttr {
            allowed_access,
            parent_fd: parent.as_raw_fd(),
        };
        // SAFETY: `rule` is a valid path-beneath attribute for this call.
        let result = unsafe {
            libc::syscall(
                libc::SYS_landlock_add_rule,
                ruleset.as_raw_fd(),
                RULE_PATH_BENEATH,
                &rule as *const PathBeneathAttr,
                0u32,
            )
        };
        if result < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    pub(super) fn ruleset(writable: &Directory) -> io::Result<OwnedFd> {
        let abi = required(abi())?;
        let attr = RulesetAttr {
            handled_access_fs: WRITE_ACCESS,
            handled_access_net: 0,
            scoped: if abi >= SCOPE_ABI { SCOPES } else { 0 },
        };
        // SAFETY: `attr` is valid for its full size; unknown tail fields are zero.
        let fd = unsafe {
            libc::syscall(
                libc::SYS_landlock_create_ruleset,
                &attr as *const RulesetAttr,
                std::mem::size_of::<RulesetAttr>(),
                0u32,
            )
        };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: the kernel returned a new close-on-exec descriptor we own.
        let ruleset = unsafe { OwnedFd::from_raw_fd(fd as RawFd) };
        add(&ruleset, writable.as_fd(), WRITE_ACCESS)?;
        let null = File::options()
            .read(true)
            .custom_flags(libc::O_PATH)
            .open("/dev/null")?;
        add(&ruleset, null.as_fd(), WRITE_FILE | TRUNCATE)?;
        Ok(ruleset)
    }

    /// Runs in the forked child; async-signal-safe.
    pub(super) fn restrict_self(ruleset: RawFd) -> io::Result<()> {
        // SAFETY: plain syscalls without memory arguments.
        unsafe {
            if libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) != 0 {
                return Err(io::Error::last_os_error());
            }
            if libc::syscall(libc::SYS_landlock_restrict_self, ruleset, 0u32) != 0 {
                return Err(io::Error::last_os_error());
            }
        }
        Ok(())
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn hosts_without_the_minimum_abi_are_refused_not_skipped() {
        for abi in [
            Ok(1),
            Ok(linux::MINIMUM_ABI - 1),
            Err(io::Error::from_raw_os_error(libc::ENOSYS)),
            Err(io::Error::from_raw_os_error(libc::EOPNOTSUPP)),
        ] {
            assert!(is_unavailable(&linux::required(abi).unwrap_err()));
        }
        assert_eq!(
            linux::required(Ok(linux::MINIMUM_ABI)).unwrap(),
            linux::MINIMUM_ABI
        );
    }

    #[test]
    fn confined_processes_write_only_beneath_the_held_directory() {
        let temp = tempfile::tempdir().unwrap();
        let held = temp.path().join("held");
        let elsewhere = temp.path().join("elsewhere");
        fs::create_dir_all(held.join("nested")).unwrap();
        fs::create_dir_all(&elsewhere).unwrap();
        fs::write(elsewhere.join("existing"), b"original").unwrap();
        let confinement = Confinement::writable_beneath(&Directory::open(&held).unwrap()).unwrap();

        let mut command = Command::new("/bin/sh");
        command.arg("-c").arg(format!(
            "echo inside > '{held}/nested/file'; \
             echo outside 2>/dev/null > '{elsewhere}/new'; \
             echo changed 2>/dev/null > '{elsewhere}/existing'; \
             mkdir '{elsewhere}/directory' 2>/dev/null; \
             ln -s / '{elsewhere}/link' 2>/dev/null; \
             sh -c \"echo child > '{elsewhere}/descendant'\" 2>/dev/null; \
             echo discarded > /dev/null; exit 0",
            held = held.display(),
            elsewhere = elsewhere.display(),
        ));
        confinement.apply(&mut command);
        assert!(command.status().unwrap().success());

        assert_eq!(fs::read(held.join("nested/file")).unwrap(), b"inside\n");
        assert_eq!(fs::read(elsewhere.join("existing")).unwrap(), b"original");
        let mut names: Vec<_> = fs::read_dir(&elsewhere)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        names.sort();
        assert_eq!(names, ["existing"]);
    }

    #[test]
    fn confinement_follows_the_held_directory_not_its_former_path() {
        let temp = tempfile::tempdir().unwrap();
        let held = temp.path().join("held");
        fs::create_dir_all(&held).unwrap();
        let confinement = Confinement::writable_beneath(&Directory::open(&held).unwrap()).unwrap();
        fs::rename(&held, temp.path().join("moved")).unwrap();
        fs::create_dir_all(&held).unwrap();

        let mut command = Command::new("/bin/sh");
        command.arg("-c").arg(format!(
            "echo path 2>/dev/null > '{}/file'; echo held > '{}/file'",
            held.display(),
            temp.path().join("moved").display()
        ));
        confinement.apply(&mut command);
        assert!(command.status().unwrap().success());

        assert!(!held.join("file").exists());
        assert_eq!(fs::read(temp.path().join("moved/file")).unwrap(), b"held\n");
    }

    #[test]
    fn confined_processes_cannot_signal_processes_outside_their_tree() {
        if linux::abi().unwrap() < linux::SCOPE_ABI {
            eprintln!("kernel lacks Landlock scopes; not applicable");
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        let confinement =
            Confinement::writable_beneath(&Directory::open(temp.path()).unwrap()).unwrap();
        let mut command = Command::new("/bin/sh");
        command.arg("-c").arg(format!(
            "kill -0 {} 2>/dev/null && exit 1; kill -0 $$",
            std::process::id()
        ));
        confinement.apply(&mut command);
        assert!(command.status().unwrap().success());
    }
}
