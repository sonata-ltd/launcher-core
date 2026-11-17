use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

use futures::{stream, StreamExt};

use crate::java::utils::{detect_properties, DetectedJavaRuntime};

mod common;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
mod windows;

#[cfg(windows)]
pub(crate) const JAVA_BIN: &str = "java.exe";
#[cfg(not(windows))]
pub(crate) const JAVA_BIN: &str = "java";

const PROBE_CONCURRENCY: usize = 8;

/// Everything the search reads from outside: injected so tests can point it at
/// a temp dir instead of `/usr/lib/jvm`.
pub struct DiscoveryContext {
    /// Prepended to every absolute system path. `/` in production.
    pub root: PathBuf,
    pub home: Option<PathBuf>,
    /// The launcher's own runtimes dir, `LauncherPaths::java_runtimes`.
    pub own_runtimes: PathBuf,
    pub env: fn(&str) -> Option<String>,
}

impl DiscoveryContext {
    pub fn system(own_runtimes: PathBuf) -> Self {
        Self {
            root: PathBuf::from("/"),
            home: home::home_dir(),
            own_runtimes,
            env: |k| std::env::var(k).ok().filter(|v| !v.is_empty()),
        }
    }

    /// `/usr/lib/jvm` -> `<root>/usr/lib/jvm`.
    pub(crate) fn sys(&self, abs: &str) -> PathBuf {
        self.root.join(abs.trim_start_matches('/'))
    }
}

/// Every path that may be a java executable. Cheap: only `read_dir` and
/// `is_file`, never runs anything. Duplicates go after canonicalization.
pub fn candidates(cx: &DiscoveryContext) -> Vec<PathBuf> {
    let mut out = Vec::new();

    common::collect(cx, &mut out);

    #[cfg(target_os = "linux")]
    linux::collect(cx, &mut out);
    #[cfg(target_os = "macos")]
    macos::collect(cx, &mut out);
    #[cfg(windows)]
    windows::collect(cx, &mut out);

    let mut seen = HashSet::new();
    out.into_iter()
        .filter(|p| p.is_file())
        .map(|p| dunce::canonicalize(&p).unwrap_or(p))
        .filter(|p| seen.insert(p.clone()))
        .collect()
}

/// Probes each candidate, `PROBE_CONCURRENCY` at a time. A candidate that
/// fails is logged and skipped, never fails the search. One result per home.
pub async fn probe_all(paths: Vec<PathBuf>) -> Vec<DetectedJavaRuntime> {
    let probed: Vec<_> = stream::iter(paths)
        .map(|p| async move {
            match detect_properties(&p).await {
                Ok(d) => Some(d),
                Err(e) => {
                    tracing::debug!(path = %p.display(), error = %e, "java candidate rejected");
                    None
                }
            }
        })
        .buffer_unordered(PROBE_CONCURRENCY)
        .filter_map(futures::future::ready)
        .collect()
        .await;

    dedupe_by_home(probed)
}

/// Several paths lead to one JDK (`/usr/bin/java` → alternatives →
/// `/usr/lib/jvm/x/bin/java`). Keep one per `java.home`, preferring the
/// executable that lives inside it.
fn dedupe_by_home(mut found: Vec<DetectedJavaRuntime>) -> Vec<DetectedJavaRuntime> {
    found.retain(|d| d.major.is_some() && d.home_path.is_some());
    found.sort_by_key(|d| !inside_home(d));

    let mut seen = HashSet::new();
    found.retain(|d| seen.insert(d.home_path.clone()));
    found
}

fn inside_home(d: &DetectedJavaRuntime) -> bool {
    d.home_path
        .as_deref()
        .is_some_and(|h| d.exec_path.starts_with(h))
}

/// `<dir>/bin/java` and the JDK 8 layout `<dir>/jre/bin/java`, for each
/// immediate subdirectory of `parent` whose name passes `filter`.
pub(crate) fn scan_homes(parent: &Path, filter: impl Fn(&str) -> bool, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(parent) else {
        return;
    };

    for entry in entries.flatten() {
        let name = entry.file_name();

        if !filter(&name.to_string_lossy()) {
            continue;
        }

        push_home(&entry.path(), out);
    }
}

pub(crate) fn push_home(home: &Path, out: &mut Vec<PathBuf>) {
    out.push(home.join("bin").join(JAVA_BIN));
    out.push(home.join("jre").join("bin").join(JAVA_BIN));
}

#[cfg(all(test, unix))]
mod tests {
    use std::{fs, os::unix::fs::symlink};

    use super::*;

    fn touch(path: &Path) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, b"").unwrap();
    }

    fn context(root: &Path) -> DiscoveryContext {
        DiscoveryContext {
            root: root.to_path_buf(),
            home: Some(root.join("home")),
            own_runtimes: root.join("own"),
            env: |_| None,
        }
    }

    #[test]
    fn candidates_are_unique_and_canonical() {
        let tmp = tempfile::tempdir().unwrap();
        let root = dunce::canonicalize(tmp.path()).unwrap();

        let own = root.join("own/temurin-21/bin/java");
        let ide = root.join("home/.jdks/corretto-17/bin/java");
        let sdk = root.join("home/.sdkman/candidates/java/21.0.5-tem/bin/java");
        let mojang = root
            .join("home/.minecraft/runtime/java-runtime-delta/linux/java-runtime-delta/bin/java");
        for path in [&own, &ide, &sdk, &mojang] {
            touch(path);
        }

        // `current` points at a sibling and must not produce a second entry.
        symlink(
            root.join("home/.sdkman/candidates/java/21.0.5-tem"),
            root.join("home/.sdkman/candidates/java/current"),
        )
        .unwrap();

        let mut expected = vec![own, ide, sdk, mojang];

        #[cfg(target_os = "linux")]
        {
            let jvm = root.join("usr/lib/jvm/java-21-openjdk/bin/java");
            touch(&jvm);
            // Distro aliases (`default`, `default-runtime`) are symlinks.
            symlink(
                root.join("usr/lib/jvm/java-21-openjdk"),
                root.join("usr/lib/jvm/default"),
            )
            .unwrap();
            expected.push(jvm);
        }

        let mut found = candidates(&context(&root));
        found.sort();
        expected.sort();

        assert_eq!(found, expected);
    }

    fn detected(exec: &str, home: &str) -> DetectedJavaRuntime {
        DetectedJavaRuntime {
            version: Some("21.0.5".into()),
            major: Some(21),
            arch: Some("amd64".into()),
            exec_path: PathBuf::from(exec),
            home_path: Some(PathBuf::from(home)),
            vendor: None,
        }
    }

    #[test]
    fn one_runtime_per_home_preferring_its_own_binary() {
        let found = dedupe_by_home(vec![
            detected("/usr/bin/java", "/usr/lib/jvm/21"),
            detected("/usr/lib/jvm/21/bin/java", "/usr/lib/jvm/21"),
            detected("/opt/jdk/17/bin/java", "/opt/jdk/17"),
        ]);

        assert_eq!(found.len(), 2);
        assert!(found
            .iter()
            .any(|d| d.exec_path == Path::new("/usr/lib/jvm/21/bin/java")));
    }

    #[test]
    fn unknown_major_is_dropped() {
        let mut odd = detected("/x/bin/java", "/x");
        odd.major = None;

        assert!(dedupe_by_home(vec![odd]).is_empty());
    }
}
