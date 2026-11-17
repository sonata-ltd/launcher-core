use std::path::PathBuf;

use super::{push_home, DiscoveryContext};

pub fn collect(cx: &DiscoveryContext, out: &mut Vec<PathBuf>) {
    // `<name>.jdk/Contents/Home` — Oracle/Temurin/Zulu .pkg and Homebrew casks.
    let mut vm_dirs = vec![
        cx.sys("/Library/Java/JavaVirtualMachines"),
        cx.sys("/System/Library/Java/JavaVirtualMachines"),
    ];
    if let Some(home) = &cx.home {
        vm_dirs.push(home.join("Library/Java/JavaVirtualMachines"));
    }

    for dir in vm_dirs {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            push_home(&entry.path().join("Contents/Home"), out);
        }
    }

    // Homebrew formulae: `openjdk`, `openjdk@17`, `openjdk@21`, …
    for prefix in ["/opt/homebrew/opt", "/usr/local/opt"] {
        let Ok(entries) = std::fs::read_dir(cx.sys(prefix)) else {
            continue;
        };
        for entry in entries.flatten() {
            if entry.file_name().to_string_lossy().starts_with("openjdk") {
                // The formula's keg also ships a full .jdk bundle.
                push_home(&entry.path().join("libexec/openjdk.jdk/Contents/Home"), out);
                push_home(&entry.path(), out);
            }
        }
    }
}
