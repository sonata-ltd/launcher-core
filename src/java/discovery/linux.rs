use std::path::PathBuf;

use super::{scan_homes, DiscoveryContext};

const JVM_DIRS: &[&str] = &[
    "/usr/lib/jvm",
    "/usr/lib64/jvm",
    "/usr/lib32/jvm",
    "/usr/java",
    "/opt/jdk",
    "/opt/jdks",
    "/opt/ibm",
    "/app/jdk", // Flatpak
];

pub fn collect(cx: &DiscoveryContext, out: &mut Vec<PathBuf>) {
    let snap = (cx.env)("SNAP").map(PathBuf::from);

    for dir in JVM_DIRS {
        scan_homes(&cx.sys(dir), |_| true, out);

        if let Some(snap) = &snap {
            scan_homes(&snap.join(dir.trim_start_matches('/')), |_| true, out);
        }
    }

    // Gentoo
    let gentoo = |n: &str| n.starts_with("openjdk-") || n.starts_with("openj9-");
    for dir in ["/usr/lib64", "/usr/lib", "/opt"] {
        scan_homes(&cx.sys(dir), gentoo, out);
    }

    // AOSC
    scan_homes(
        &cx.sys("/usr/lib"),
        |n| n == "java" || n.starts_with("java-"),
        out,
    );
}
