use std::{
    ffi::OsStr,
    fs::{self, File},
    io,
    path::{Path, PathBuf},
    sync::Arc,
};

use super::identity::FileIdentity;

/// An opened directory used for publication. On Windows the parent chain
/// stays open without delete sharing for the lifetime of each child.
#[derive(Debug)]
pub(crate) struct Directory {
    handle: File,
    path: PathBuf,
    #[cfg(windows)]
    _parent: Option<Arc<Directory>>,
}

impl Directory {
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn open(path: &Path) -> io::Result<Arc<Self>> {
        let mut options = fs::OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW);
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            use windows_sys::Win32::Storage::FileSystem::{
                FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_READ_ATTRIBUTES,
                FILE_SHARE_READ, FILE_SHARE_WRITE,
            };
            options
                .access_mode(FILE_READ_ATTRIBUTES)
                .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
                .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT);
        }
        let handle = options.open(path)?;
        let metadata = handle.metadata()?;
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;
            if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
                return Err(io::Error::other("publication directory is redirected"));
            }
        }
        if !metadata.is_dir() {
            return Err(io::ErrorKind::NotADirectory.into());
        }
        Ok(Arc::new(Self {
            handle,
            path: path.to_owned(),
            #[cfg(windows)]
            _parent: None,
        }))
    }

    pub(crate) fn open_relative(self: &Arc<Self>, relative: &Path) -> io::Result<Arc<Self>> {
        super::relative(relative).map_err(io::Error::other)?;
        let mut directory = self.clone();
        for component in relative.components() {
            directory = directory.child(component.as_os_str())?;
        }
        Ok(directory)
    }

    pub(crate) fn create_relative(self: &Arc<Self>, relative: &Path) -> io::Result<Arc<Self>> {
        super::relative(relative).map_err(io::Error::other)?;
        let mut directory = self.clone();
        for component in relative.components() {
            directory = directory.create_child(component.as_os_str())?;
        }
        Ok(directory)
    }

    pub(crate) fn create_entry_parent(self: &Arc<Self>, entry: &Path) -> io::Result<Arc<Self>> {
        super::relative(entry).map_err(io::Error::other)?;
        match entry.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => self.create_relative(parent),
            _ => Ok(self.clone()),
        }
    }

    pub(crate) fn create_file(&self, name: &OsStr) -> io::Result<File> {
        self.create_file_with_mode(name, 0o666)
    }

    fn create_file_with_mode(&self, name: &OsStr, mode: u32) -> io::Result<File> {
        self.verify_location()?;
        #[cfg(unix)]
        {
            use std::os::fd::{AsRawFd, FromRawFd};
            let name = component(name)?;
            let fd = unsafe {
                libc::openat(
                    self.handle.as_raw_fd(),
                    name.as_ptr(),
                    libc::O_WRONLY
                        | libc::O_CREAT
                        | libc::O_EXCL
                        | libc::O_NOFOLLOW
                        | libc::O_CLOEXEC,
                    mode as libc::mode_t,
                )
            };
            if fd < 0 {
                return Err(io::Error::last_os_error());
            }
            // openat returned a new descriptor owned by this file.
            Ok(unsafe { File::from_raw_fd(fd) })
        }
        #[cfg(windows)]
        {
            let _ = mode;
            windows_component(name)?;
            fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(true)
                .open(self.path.join(name))
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = (name, mode);
            Err(io::ErrorKind::Unsupported.into())
        }
    }

    pub(crate) fn with_staged_file<W, T, E: From<io::Error>>(
        &self,
        write: impl FnOnce(&mut File) -> Result<W, E>,
        publish: impl FnOnce(&OsStr, W) -> Result<T, E>,
    ) -> Result<T, E> {
        let name = format!(".sonata-staged-{}", uuid::Uuid::new_v4());
        let name = OsStr::new(&name);
        let mut file = self.create_file_with_mode(name, 0o600)?;
        let identity = FileIdentity::from_handle(&file)?;
        let written = write(&mut file);
        drop(file);
        let check_identity = || -> io::Result<()> {
            self.verify_location()?;
            if FileIdentity::at(&self.path.join(name))? != identity {
                return Err(io::Error::other("staged file changed"));
            }
            Ok(())
        };
        let result = written.and_then(|value| {
            check_identity()?;
            publish(name, value)
        });
        if check_identity().is_ok() {
            let _ = self.unlink(name);
        }
        result
    }

    pub(crate) fn hard_link(&self, source: &OsStr, name: &OsStr) -> io::Result<()> {
        self.link_file(source, self, name)
    }

    pub(crate) fn link_file(
        &self,
        source: &OsStr,
        destination: &Self,
        name: &OsStr,
    ) -> io::Result<()> {
        self.verify_location()?;
        destination.verify_location()?;
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            let source = component(source)?;
            let name = component(name)?;
            if unsafe {
                libc::linkat(
                    self.handle.as_raw_fd(),
                    source.as_ptr(),
                    destination.handle.as_raw_fd(),
                    name.as_ptr(),
                    0,
                )
            } != 0
            {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        }
        #[cfg(not(unix))]
        {
            #[cfg(windows)]
            {
                windows_component(source)?;
                windows_component(name)?;
            }
            if Path::new(source).file_name() != Some(source)
                || Path::new(name).file_name() != Some(name)
            {
                return Err(io::ErrorKind::InvalidInput.into());
            }
            fs::hard_link(self.path.join(source), destination.path.join(name))
        }
    }

    pub(crate) fn create_symlink(&self, target: &Path, name: &OsStr) -> io::Result<()> {
        self.verify_location()?;
        #[cfg(unix)]
        {
            use std::os::{fd::AsRawFd, unix::ffi::OsStrExt};
            let target = std::ffi::CString::new(target.as_os_str().as_bytes())?;
            let name = component(name)?;
            if unsafe { libc::symlinkat(target.as_ptr(), self.handle.as_raw_fd(), name.as_ptr()) }
                != 0
            {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        }
        #[cfg(windows)]
        {
            windows_component(name)?;
            std::os::windows::fs::symlink_file(target, self.path.join(name))
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = (target, name);
            Err(io::ErrorKind::Unsupported.into())
        }
    }

    #[cfg(unix)]
    pub(crate) fn set_permissions(&self, permissions: fs::Permissions) -> io::Result<()> {
        self.handle.set_permissions(permissions)
    }

    pub(crate) fn create_child(self: &Arc<Self>, name: &OsStr) -> io::Result<Arc<Self>> {
        match self.mkdir(name, false) {
            Ok(()) => (),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => (),
            Err(error) => return Err(error),
        }
        self.child(name)
    }

    pub(crate) fn create_private_child(self: &Arc<Self>, name: &OsStr) -> io::Result<Arc<Self>> {
        self.verify_location()?;
        self.mkdir(name, true)?;
        self.child(name)
    }

    fn mkdir(&self, name: &OsStr, private: bool) -> io::Result<()> {
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            let name_bytes = component(name)?;
            let mode = if private { 0o700 } else { 0o777 };
            if unsafe { libc::mkdirat(self.handle.as_raw_fd(), name_bytes.as_ptr(), mode) } != 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        }
        #[cfg(not(unix))]
        {
            // Windows inherits the ACL of the trusted scratch root.
            let _ = private;
            #[cfg(windows)]
            windows_component(name)?;
            if Path::new(name).file_name() != Some(name) {
                return Err(io::ErrorKind::InvalidInput.into());
            }
            fs::create_dir(self.path.join(name))
        }
    }

    #[cfg(unix)]
    fn child(self: &Arc<Self>, name: &OsStr) -> io::Result<Arc<Self>> {
        use std::os::fd::{AsRawFd, FromRawFd};
        let name_bytes = component(name)?;
        // The caller owns the parent descriptor. openat returns a new fd.
        let fd = unsafe {
            libc::openat(
                self.handle.as_raw_fd(),
                name_bytes.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Arc::new(Self {
            handle: unsafe { File::from_raw_fd(fd) },
            path: self.path.join(name),
        }))
    }

    #[cfg(windows)]
    fn child(self: &Arc<Self>, name: &OsStr) -> io::Result<Arc<Self>> {
        windows_component(name)?;
        let child = Self::open(&self.path.join(name))?;
        let handle = child.handle.try_clone()?;
        Ok(Arc::new(Self {
            handle,
            path: child.path.clone(),
            _parent: Some(self.clone()),
        }))
    }

    #[cfg(not(any(unix, windows)))]
    fn child(self: &Arc<Self>, _: &OsStr) -> io::Result<Arc<Self>> {
        Err(io::ErrorKind::Unsupported.into())
    }

    pub(crate) fn verify_location(&self) -> io::Result<()> {
        if FileIdentity::at(&self.path)? != FileIdentity::from_handle(&self.handle)? {
            return Err(io::Error::other("publication directory changed"));
        }
        Ok(())
    }

    pub(crate) fn unlink(&self, name: &OsStr) -> io::Result<()> {
        self.verify_location()?;
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            let name = component(name)?;
            if unsafe { libc::unlinkat(self.handle.as_raw_fd(), name.as_ptr(), 0) } != 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        }
        #[cfg(not(unix))]
        {
            if Path::new(name).file_name() != Some(name) {
                return Err(io::ErrorKind::InvalidInput.into());
            }
            fs::remove_file(self.path.join(name))
        }
    }

    pub(crate) fn replace(&self, source: &OsStr, destination: &OsStr) -> io::Result<()> {
        self.verify_location()?;
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            let source = component(source)?;
            let destination = component(destination)?;
            let dir = self.handle.as_raw_fd();
            if unsafe { libc::renameat(dir, source.as_ptr(), dir, destination.as_ptr()) } != 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        }
        #[cfg(not(unix))]
        {
            #[cfg(windows)]
            {
                windows_component(source)?;
                windows_component(destination)?;
            }
            if Path::new(source).file_name() != Some(source)
                || Path::new(destination).file_name() != Some(destination)
            {
                return Err(io::ErrorKind::InvalidInput.into());
            }
            fs::rename(self.path.join(source), self.path.join(destination))
        }
    }

    pub(crate) fn publish(
        &self,
        source: &OsStr,
        destination: &Directory,
        name: &OsStr,
    ) -> io::Result<()> {
        if Path::new(source).file_name() != Some(source)
            || Path::new(name).file_name() != Some(name)
        {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        self.verify_location()?;
        destination.verify_location()?;
        self.publish_relative(source, destination, name)
    }

    #[cfg(unix)]
    fn publish_relative(
        &self,
        source: &OsStr,
        destination: &Directory,
        name: &OsStr,
    ) -> io::Result<()> {
        use std::os::fd::AsRawFd;
        let source = component(source)?;
        let name = component(name)?;
        let from = self.handle.as_raw_fd();
        let to = destination.handle.as_raw_fd();
        #[cfg(target_os = "linux")]
        {
            let renamed = unsafe {
                libc::renameat2(
                    from,
                    source.as_ptr(),
                    to,
                    name.as_ptr(),
                    libc::RENAME_NOREPLACE,
                )
            };
            if renamed == 0 {
                return Ok(());
            }
            let error = io::Error::last_os_error();
            if !matches!(
                error.raw_os_error(),
                Some(libc::ENOSYS | libc::EINVAL | libc::EOPNOTSUPP)
            ) {
                return Err(error);
            }
        }
        #[cfg(target_os = "macos")]
        {
            let renamed = unsafe {
                libc::renameatx_np(from, source.as_ptr(), to, name.as_ptr(), libc::RENAME_EXCL)
            };
            if renamed == 0 {
                return Ok(());
            }
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::Unsupported {
                return Err(error);
            }
        }
        // linkat fails if the destination exists. The staging owner is
        // responsible for removing the original name after publication.
        if unsafe { libc::linkat(from, source.as_ptr(), to, name.as_ptr(), 0) } == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }

    #[cfg(not(unix))]
    fn publish_relative(
        &self,
        source: &OsStr,
        destination: &Directory,
        name: &OsStr,
    ) -> io::Result<()> {
        super::relative(Path::new(source)).map_err(io::Error::other)?;
        super::relative(Path::new(name)).map_err(io::Error::other)?;
        super::publish::publish_new(self.path.join(source), destination.path.join(name))
    }
}

#[cfg(windows)]
fn windows_component(name: &OsStr) -> io::Result<()> {
    let name = name.to_str().ok_or(io::ErrorKind::InvalidInput)?;
    super::portable_name::validate_file_name(name)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn directory_publication_declines_a_changed_destination() {
        let root = tempfile::tempdir().unwrap();
        let anchor = Directory::open(root.path()).unwrap();
        let staged = anchor.create_child(OsStr::new("staged")).unwrap();
        let target = anchor.create_child(OsStr::new("target")).unwrap();
        fs::write(root.path().join("staged/artifact"), b"staged").unwrap();
        fs::rename(root.path().join("target"), root.path().join("previous")).unwrap();
        fs::create_dir(root.path().join("target")).unwrap();
        assert!(staged
            .publish(OsStr::new("artifact"), &target, OsStr::new("artifact"))
            .is_err());
        assert!(!root.path().join("target/artifact").exists());
        assert_eq!(
            fs::read(root.path().join("staged/artifact")).unwrap(),
            b"staged"
        );
    }
}

#[cfg(unix)]
fn component(name: &OsStr) -> io::Result<std::ffi::CString> {
    use std::os::unix::ffi::OsStrExt;
    let path = Path::new(name);
    super::relative(path).map_err(io::Error::other)?;
    if path.file_name() != Some(name) {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    std::ffi::CString::new(name.as_bytes()).map_err(io::Error::other)
}
