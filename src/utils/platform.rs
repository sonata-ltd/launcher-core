use std::sync::{LazyLock, OnceLock};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Os {
    Linux,
    MacOs,
    Windows,
    Other(&'static str),
}

impl Os {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Linux => "linux",
            Self::MacOs => "macos",
            Self::Windows => "windows",
            Self::Other(name) => name,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Arch<'a> {
    X86,
    X86_64,
    Arm,
    Aarch64,
    Other(&'a str),
}

impl<'a> Arch<'a> {
    pub(crate) fn parse(name: &'a str) -> Self {
        match name {
            "amd64" | "x86_64" => Self::X86_64,
            "aarch64" | "arm64" => Self::Aarch64,
            "x86" | "i386" | "i486" | "i586" | "i686" => Self::X86,
            "arm" | "armv7" | "armv7l" => Self::Arm,
            other => Self::Other(other),
        }
    }

    pub(crate) fn name(self) -> &'a str {
        match self {
            Self::X86 => "x86",
            Self::X86_64 => "x86_64",
            Self::Arm => "arm",
            Self::Aarch64 => "aarch64",
            Self::Other(name) => name,
        }
    }
}

pub(crate) struct HostPlatform {
    pub os: Os,
    pub arch: Arch<'static>,
    pub musl: bool,
    version: OnceLock<String>,
}

impl HostPlatform {
    pub(crate) fn current() -> &'static Self {
        static HOST: LazyLock<HostPlatform> = LazyLock::new(|| HostPlatform {
            os: match std::env::consts::OS {
                "linux" => Os::Linux,
                "macos" => Os::MacOs,
                "windows" => Os::Windows,
                other => Os::Other(other),
            },
            arch: Arch::parse(std::env::consts::ARCH),
            musl: cfg!(target_env = "musl"),
            version: OnceLock::new(),
        });
        &HOST
    }

    pub(crate) fn os_version(&self) -> &str {
        self.version.get_or_init(os_version)
    }
}

fn os_version() -> String {
    #[cfg(target_os = "linux")]
    {
        std::fs::read_to_string("/proc/sys/kernel/osrelease")
            .unwrap_or_default()
            .trim()
            .into()
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("sw_vers")
            .arg("-productVersion")
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().into())
            .unwrap_or_default()
    }
    #[cfg(windows)]
    {
        windows_registry::LOCAL_MACHINE
            .open("SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion")
            .ok()
            .and_then(|key| key.get_string("CurrentBuildNumber").ok())
            .map(|v| format!("10.0.{v}"))
            .unwrap_or_default()
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    {
        String::new()
    }
}
