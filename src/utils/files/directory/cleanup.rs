use super::{Directory, FileIdentity};
use std::{ffi::OsStr, fs, io, sync::Arc};

impl Directory {
    pub(crate) fn move_child_to(
        &self,
        child: Arc<Self>,
        destination: &Arc<Self>,
        name: &OsStr,
    ) -> io::Result<Arc<Self>> {
        let source = self.verify_child(&child)?.to_owned();
        let witness = child.move_witness()?;
        drop(child);
        let identity = FileIdentity::from_handle(&witness)?;
        self.publish_directory(&source, identity, destination, name)?;
        let published = destination.child(name)?;
        if published.identity()? != identity {
            return Err(io::Error::other("moved directory changed"));
        }
        Ok(published)
    }

    /// The caller establishes ownership before opening a tree for cleanup.
    /// Every mutation is made from an opened parent. Keep the owner's state
    /// and allow replay when an entry cannot be verified or removed.
    pub(crate) fn remove_child_tree(&self, child: Arc<Self>) -> io::Result<()> {
        self.remove_tree(child, None)
    }

    pub(crate) fn remove_marked_tree(&self, child: Arc<Self>, marker: &OsStr) -> io::Result<()> {
        self.remove_tree(child, Some(marker))
    }

    fn remove_tree(&self, child: Arc<Self>, marker: Option<&OsStr>) -> io::Result<()> {
        self.verify_child(&child)?;
        let mut owner = match marker {
            Some(name) => match child.open_file(name) {
                Ok(file) => Some((name, file)),
                Err(error) if error.kind() == io::ErrorKind::NotFound => None,
                Err(error) => return Err(error),
            },
            None => None,
        };
        let mut pending = vec![(open_entries(&child)?, child)];
        while !pending.is_empty() {
            let root = pending.len() == 1;
            let Some((entries, directory)) = pending.last_mut() else {
                break;
            };
            if let Some(entry) = entries.next() {
                let entry = entry?;
                let name = entry.file_name();
                if root && marker == Some(name.as_os_str()) {
                    continue;
                }
                let kind = entry.file_type()?;
                if kind.is_dir() {
                    let child = directory.child(&name)?;
                    pending.push((open_entries(&child)?, child));
                } else if kind.is_file() {
                    directory.remove_regular(&name)?;
                } else if kind.is_symlink() {
                    directory.remove_link(&name)?;
                } else {
                    return Err(io::Error::other("unrecognized entry in owned directory"));
                }
                continue;
            }
            let Some((entries, empty)) = pending.pop() else {
                break;
            };
            drop(entries);
            if root {
                if let Some((name, owner)) = owner.take() {
                    empty.verify_file(name, FileIdentity::from_handle(&owner)?)?;
                    empty.unlink(name)?;
                }
            }
            let parent = pending.last().map_or(self, |(_, parent)| parent.as_ref());
            parent.remove_empty_child(empty)?;
        }
        Ok(())
    }

    pub(crate) fn remove_regular(&self, name: &OsStr) -> io::Result<()> {
        self.remove_verified_file(name, |_| Ok(()))
    }

    pub(crate) fn remove_verified_file(
        &self,
        name: &OsStr,
        verify: impl FnOnce(&fs::File) -> io::Result<()>,
    ) -> io::Result<()> {
        let file = self.open_file(name)?;
        verify(&file)?;
        self.verify_file(name, FileIdentity::from_handle(&file)?)?;
        self.unlink(name)
    }

    pub(crate) fn remove_empty_child(&self, child: Arc<Self>) -> io::Result<()> {
        let name = self.verify_child(&child)?.to_owned();
        let witness = child.move_witness()?;
        drop(child);
        self.verify_file(&name, FileIdentity::from_handle(&witness)?)?;
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            let name = super::component(&name)?;
            // Invoke unlinkat against the live parent and a single name.
            if unsafe { libc::unlinkat(self.handle.as_raw_fd(), name.as_ptr(), libc::AT_REMOVEDIR) }
                != 0
            {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        }
        #[cfg(not(unix))]
        fs::remove_dir(self.path().join(&name))
    }

    fn verify_child<'a>(&self, child: &'a Directory) -> io::Result<&'a OsStr> {
        let path = child.path();
        if path.parent() != Some(self.path()) {
            return Err(io::Error::other("cleanup target is not an opened child"));
        }
        let name = path.file_name().ok_or(io::ErrorKind::InvalidInput)?;
        self.verify_file(name, child.identity()?)?;
        Ok(name)
    }

    fn remove_link(&self, name: &OsStr) -> io::Result<()> {
        self.verify_location()?;
        let path = self.path().join(name);
        let identity = FileIdentity::at(&path)?;
        let kind = fs::symlink_metadata(&path)?.file_type();
        if !kind.is_symlink() {
            return Err(io::Error::other("link changed during cleanup"));
        }
        self.verify_file(name, identity)?;
        #[cfg(windows)]
        {
            use std::os::windows::fs::FileTypeExt;
            if kind.is_symlink_dir() {
                return fs::remove_dir(path);
            }
        }
        self.unlink(name)
    }
}

fn open_entries(directory: &Directory) -> io::Result<fs::ReadDir> {
    directory.verify_location()?;
    let entries = fs::read_dir(directory.path())?;
    directory.verify_location()?;
    Ok(entries)
}
