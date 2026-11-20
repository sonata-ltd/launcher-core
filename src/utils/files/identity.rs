use std::{fs, io, path::Path};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct FileIdentity {
    device: u64,
    file: u128,
}

impl FileIdentity {
    pub(crate) fn storage_key(self) -> String {
        format!("{:016x}:{:032x}", self.device, self.file)
    }

    #[cfg(unix)]
    pub(crate) fn at(path: &Path) -> io::Result<Self> {
        Ok(Self::from_metadata(&fs::symlink_metadata(path)?))
    }

    #[cfg(unix)]
    pub(crate) fn from_handle(file: &fs::File) -> io::Result<Self> {
        Ok(Self::from_metadata(&file.metadata()?))
    }

    #[cfg(unix)]
    fn from_metadata(metadata: &fs::Metadata) -> Self {
        use std::os::unix::fs::MetadataExt;
        Self {
            device: metadata.dev(),
            file: metadata.ino().into(),
        }
    }

    #[cfg(windows)]
    pub(crate) fn at(path: &Path) -> io::Result<Self> {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_READ_ATTRIBUTES,
            FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
        };
        let file = fs::OpenOptions::new()
            .access_mode(FILE_READ_ATTRIBUTES)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .open(path)?;
        Self::from_handle(&file)
    }

    #[cfg(windows)]
    pub(crate) fn from_handle(file: &fs::File) -> io::Result<Self> {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Storage::FileSystem::{
            FileIdInfo, GetFileInformationByHandleEx, FILE_ID_INFO,
        };
        let mut info: FILE_ID_INFO = Default::default();
        // The open handle and correctly sized output buffer live through the call.
        let queried = unsafe {
            GetFileInformationByHandleEx(
                file.as_raw_handle(),
                FileIdInfo,
                (&mut info as *mut FILE_ID_INFO).cast(),
                size_of::<FILE_ID_INFO>() as u32,
            )
        };
        if queried == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self {
            device: info.VolumeSerialNumber,
            file: u128::from_le_bytes(info.FileId.Identifier),
        })
    }

    #[cfg(not(any(unix, windows)))]
    pub(crate) fn at(_: &Path) -> io::Result<Self> {
        Err(io::ErrorKind::Unsupported.into())
    }

    #[cfg(not(any(unix, windows)))]
    pub(crate) fn from_handle(_: &fs::File) -> io::Result<Self> {
        Err(io::ErrorKind::Unsupported.into())
    }
}
