use thiserror::Error;

#[derive(Debug)]
pub enum OsType {
    Linux,
    MacOS,
    Windows
}

#[derive(Debug)]
pub enum ArchType {
    X86,
    X86_64,
    Arm32,
    Arm64
}

#[derive(Error, Debug)]
pub enum SystemSupportError {
    #[error("OS is not supported")]
    OsNotSupported,

    #[error("CPU architecture is not supported")]
    ArchNotSupported
}

pub fn detect_os() -> Result<OsType, SystemSupportError> {
    #[cfg(target_os = "linux")]
    return Ok(OsType::Linux);

    #[cfg(target_os = "macos")]
    return Ok(OsType::MacOS);

    #[cfg(target_os = "windows")]
    return Ok(OsType::Windows);

    Err(SystemSupportError::OsNotSupported)
}

pub fn detect_arch() -> Result<ArchType, SystemSupportError> {
    #[cfg(target_arch = "x86")]
    return Ok(ArchType::X86);

    #[cfg(target_arch = "x86_64")]
    return Ok(ArchType::X86_64);

    #[cfg(target_arch = "arm")]
    return Ok(ArchType::Arm32);

    #[cfg(target_arch = "aarch64")]
    return Ok(ArchType::Arm64);

    Err(SystemSupportError::ArchNotSupported)
}
