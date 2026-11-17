use std::path::{Path, PathBuf};

use super::{push_home, scan_homes, DiscoveryContext, JAVA_BIN};

pub const EXTRA_PATHS_ENV: &str = "SONATA_JAVA_PATHS";

pub fn collect(cx: &DiscoveryContext, out: &mut Vec<PathBuf>) {
    if let Some(home) = (cx.env)("JAVA_HOME") {
        push_home(Path::new(&home), out);
    }

    if let Some(path) = (cx.env)("PATH") {
        for dir in std::env::split_paths(&path) {
            #[cfg(target_os = "macos")]
            if dir == Path::new("/usr/bin") {
                continue;
            }

            out.push(dir.join(JAVA_BIN));
        }
    }

    if let Some(extra) = (cx.env)(EXTRA_PATHS_ENV) {
        for dir in std::env::split_paths(&extra) {
            push_home(&dir, out);
            scan_homes(&dir, |_| true, out);
        }
    }

    // Managed installations are registered only after explicit validation.
    scan_homes(&cx.own_runtimes, |n| n != "managed" && n != ".managed", out);

    let Some(home) = cx.home.as_deref() else {
        return;
    };

    // IntelliJ
    scan_homes(&home.join(".jdks"), |_| true, out);

    // SDKMAN, asdf, Gradle toolchains
    let sdkman = (cx.env)("SDKMAN_DIR")
        .map(PathBuf::from)
        .unwrap_or(home.join(".sdkman"));
    scan_homes(&sdkman.join("candidates/java"), |n| n != "current", out);

    let asdf = (cx.env)("ASDF_DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or(home.join(".asdf"));
    scan_homes(&asdf.join("installs/java"), |_| true, out);

    let gradle = (cx.env)("GRADLE_USER_HOME")
        .map(PathBuf::from)
        .unwrap_or(home.join(".gradle"));
    scan_homes(&gradle.join("jdks"), |_| true, out);

    for runtime in minecraft_runtime_dirs(cx, home) {
        find_bin_recursive(&runtime, 4, out);
    }
}

/// Runtimes the official Mojang launcher downloaded:
/// `<runtime>/<component>/<os>/<component>/bin/java`.
fn minecraft_runtime_dirs(cx: &DiscoveryContext, home: &Path) -> Vec<PathBuf> {
    let mut dirs = Vec::new();

    #[cfg(target_os = "linux")]
    dirs.push(home.join(".minecraft/runtime"));

    #[cfg(target_os = "macos")]
    dirs.push(home.join("Library/Application Support/minecraft/runtime"));

    #[cfg(windows)]
    {
        if let Some(appdata) = (cx.env)("APPDATA") {
            dirs.push(PathBuf::from(appdata).join(".minecraft\\runtime"));
        }
        if let Some(local) = (cx.env)("LOCALAPPDATA") {
            dirs.push(PathBuf::from(local).join(
                "Packages\\Microsoft.4297127D64EC6_8wekyb3d8bbwe\\LocalCache\\Local\\runtime",
            ));
        }
    }

    let _ = (cx, home);
    dirs
}

/// Walks `dir` up to `depth` levels for a `bin/<java>` file.
fn find_bin_recursive(dir: &Path, depth: u32, out: &mut Vec<PathBuf>) {
    let candidate = dir.join("bin").join(JAVA_BIN);
    if candidate.is_file() {
        out.push(candidate);
        return;
    }
    if depth == 0 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        if entry.file_type().is_ok_and(|t| t.is_dir()) {
            find_bin_recursive(&entry.path(), depth - 1, out);
        }
    }
}
