use std::path::{Path, PathBuf};

use windows_registry::{Key, CURRENT_USER, LOCAL_MACHINE};

use super::{push_home, scan_homes, DiscoveryContext};

const KEY_WOW64_64KEY: u32 = 0x0100;
const KEY_WOW64_32KEY: u32 = 0x0200;

/// (key under `SOFTWARE\`, value holding the home, suffix to strip off the
/// subkey before reading — vendors that nest `hotspot\MSI` under the version).
const REGISTRY: &[(&str, &str, &str)] = &[
    (r"JavaSoft\Java Runtime Environment", "JavaHome", ""),
    (r"JavaSoft\Java Development Kit", "JavaHome", ""),
    (r"JavaSoft\JRE", "JavaHome", ""),
    (r"JavaSoft\JDK", "JavaHome", ""),
    (r"AdoptOpenJDK\JRE", "Path", r"\hotspot\MSI"),
    (r"AdoptOpenJDK\JDK", "Path", r"\hotspot\MSI"),
    (r"Eclipse Foundation\JDK", "Path", r"\hotspot\MSI"),
    (r"Eclipse Adoptium\JRE", "Path", r"\hotspot\MSI"),
    (r"Eclipse Adoptium\JDK", "Path", r"\hotspot\MSI"),
    (r"Semeru\JRE", "Path", r"\openj9\MSI"),
    (r"Semeru\JDK", "Path", r"\openj9\MSI"),
    (r"Microsoft\JDK", "Path", r"\hotspot\MSI"),
    (r"Azul Systems\Zulu", "InstallationPath", ""),
    (r"BellSoft\Liberica", "InstallationPath", ""),
];

pub fn collect(cx: &DiscoveryContext, out: &mut Vec<PathBuf>) {
    for hive in [LOCAL_MACHINE, CURRENT_USER] {
        for view in [KEY_WOW64_64KEY, KEY_WOW64_32KEY] {
            for (key, value, suffix) in REGISTRY {
                read_vendor(hive, view, key, value, suffix, out);
            }
        }
    }

    // Installs without registry keys (ZIP unpacked into the usual places).
    for var in ["ProgramFiles", "ProgramFiles(x86)", "ProgramW6432"] {
        let Some(pf) = (cx.env)(var) else { continue };
        for vendor in [
            "Java",
            "Eclipse Adoptium",
            "Eclipse Foundation",
            "AdoptOpenJDK",
            "Zulu",
            "Microsoft",
            "BellSoft",
            "Amazon Corretto",
            "Semeru",
        ] {
            scan_homes(&Path::new(&pf).join(vendor), |_| true, out);
        }
    }
}

fn read_vendor(
    hive: &Key,
    view: u32,
    key: &str,
    value: &str,
    suffix: &str,
    out: &mut Vec<PathBuf>,
) {
    let path = format!(r"SOFTWARE\{key}");
    let Ok(root) = hive.options().read().access(view).open(&path) else {
        return;
    };
    let Ok(versions) = root.keys() else { return };

    for version in versions {
        let sub = format!(r"{path}\{version}{suffix}");
        let Ok(k) = hive.options().read().access(view).open(&sub) else {
            continue;
        };
        if let Ok(home) = k.get_string(value) {
            push_home(Path::new(home.trim_end_matches('\\')), out);
        }
    }
}
