use crate::utils::platform::{Arch, HostPlatform, Os};

pub fn manifest_osarch() -> Option<&'static str> {
    let host = HostPlatform::current();
    match (host.os, host.arch) {
        (Os::Linux, Arch::X86) => Some("linux-i386"),
        (Os::Linux, Arch::X86_64) => Some("linux"),
        (Os::MacOs, Arch::X86 | Arch::X86_64) => Some("mac-os"),
        (Os::MacOs, Arch::Aarch64) => Some("mac-os-arm64"),
        (Os::Windows, Arch::X86) => Some("windows-x32"),
        (Os::Windows, Arch::X86_64) => Some("windows-x64"),
        (Os::Windows, Arch::Aarch64) => Some("windows-arm64"),
        _ => {
            tracing::error!(os = host.os.name(), arch = host.arch.name(), "unsupported Java manifest platform");
            None
        }
    }
}
