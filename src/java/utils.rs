use std::{
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

use async_std::{fs, future, io, process::Command};
use futures::AsyncReadExt;
use serde::Serialize;

use crate::java::version::parse_major;

const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Serialize)]
pub struct DetectedJavaRuntime {
    pub version: Option<String>,
    pub major: Option<u32>,
    pub arch: Option<String>,
    pub exec_path: PathBuf,
    pub home_path: Option<PathBuf>,
    pub vendor: Option<String>,
}

/// Unscoped external-runtime probe. A path is not a managed retention pin;
/// core managed consumers use the retained variant before touching a home.
pub async fn detect_properties(input: impl AsRef<Path>) -> io::Result<DetectedJavaRuntime> {
    detect_properties_impl(input.as_ref(), None).await
}

pub(crate) async fn detect_properties_retained(
    input: impl AsRef<Path>,
    java_use: &super::managed::process::JavaUse,
) -> io::Result<DetectedJavaRuntime> {
    let pin = java_use.child_pin()?;
    detect_properties_impl(input.as_ref(), Some(&pin)).await
}

async fn detect_properties_impl(
    input: &Path,
    pin: Option<&crate::utils::process_pin::ProcessPin>,
) -> io::Result<DetectedJavaRuntime> {
    let exec_path = dunce::canonicalize(input).unwrap_or_else(|_| input.to_path_buf());

    if let Some(pin) = pin {
        crate::utils::process_pin::prepare_spawn(&exec_path, &[pin])?;
    }
    let mut cmd = Command::new(&exec_path);
    cmd.args(["-XshowSettings:properties", "-version"])
        .stdin(Stdio::null());

    #[cfg(windows)]
    {
        use async_std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    let output = run_prepared_probe(&mut cmd, PROBE_TIMEOUT, pin).await?;

    // java -version writes only to stderr but we still have to check the stdout
    let output = {
        let mut s = String::new();
        s.push_str(&String::from_utf8_lossy(&output.stdout));
        s.push('\n');
        s.push_str(&String::from_utf8_lossy(&output.stderr));
        s
    };

    // Canonical, so two paths into one JDK (macOS `/Library` vs `/private/…`,
    // symlinked homes) compare equal when the search dedupes by home.
    let home_path = parse_option(&output, "java.home")
        .map(PathBuf::from)
        .map(|p| dunce::canonicalize(&p).unwrap_or(p));
    let mut version = parse_option(&output, "java.runtime.version");
    let mut vendor = parse_option(&output, "java.vendor");
    let spec_version = parse_option(&output, "java.specification.version");
    let arch = parse_option(&output, "os.arch");

    if version.is_none() || vendor.is_none() {
        if let Some(ref java_home) = home_path {
            let release_file = java_home.join("release");
            if release_file.exists() {
                if let Ok(s) = fs::read_to_string(release_file).await {
                    if version.is_none() {
                        if let Some(v) = parse_release_field(&s, "JAVA_VERSION") {
                            version = Some(v);
                        }
                    }
                    if vendor.is_none() {
                        if let Some(v) = parse_release_field(&s, "IMPLEMENTOR") {
                            vendor = Some(v);
                        }
                    }
                }
            }
        }
    }

    let major = spec_version
        .as_deref()
        .and_then(parse_major)
        .or_else(|| version.as_deref().and_then(parse_major));

    Ok(DetectedJavaRuntime {
        version,
        major,
        arch,
        exec_path,
        home_path,
        vendor,
    })
}

#[cfg(test)]
async fn run_with_timeout_retained(
    cmd: &mut Command,
    dur: Duration,
    pin: Option<&crate::utils::process_pin::ProcessPin>,
) -> io::Result<std::process::Output> {
    if let Some(pin) = pin {
        pin.before_spawn()?;
    }
    run_prepared_probe(cmd, dur, pin).await
}

#[cfg(test)]
async fn run_with_timeout(cmd: &mut Command, dur: Duration) -> io::Result<std::process::Output> {
    run_prepared_probe(cmd, dur, None).await
}

async fn run_prepared_probe(
    cmd: &mut Command,
    dur: Duration,
    pin: Option<&crate::utils::process_pin::ProcessPin>,
) -> io::Result<std::process::Output> {
    cmd.kill_on_drop(true)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn()?;
    if let Some(pin) = pin {
        pin.record_child(child.id());
    }
    let stdout = child.stdout.take().expect("stdout is piped");
    let stderr = child.stderr.take().expect("stderr is piped");
    let mut stdout_bytes = Vec::new();
    let mut stderr_bytes = Vec::new();
    let result = future::timeout(dur, async {
        let (status, stdout_result, stderr_result) = futures::join!(
            child.status(),
            read_diagnostic(stdout, &mut stdout_bytes),
            read_diagnostic(stderr, &mut stderr_bytes)
        );
        stdout_result?;
        stderr_result?;
        status
    })
    .await;
    let status = match result {
        Ok(Ok(status)) => status,
        failure => {
            let _ = child.kill();
            let exited = child.status().await.is_ok();
            if let Some(pin) = pin {
                pin.finish(exited);
            }
            let (kind, reason) = match failure {
                Ok(Err(error)) => (error.kind(), error.to_string()),
                _ => (io::ErrorKind::TimedOut, "Java process timed out".into()),
            };
            return Err(io::Error::new(
                kind,
                format!(
                    "{reason}\nstdout:\n{}\nstderr:\n{}",
                    String::from_utf8_lossy(&stdout_bytes),
                    String::from_utf8_lossy(&stderr_bytes)
                ),
            ));
        }
    };
    if let Some(pin) = pin {
        pin.finish(true);
    }
    let output = std::process::Output {
        status,
        stdout: stdout_bytes,
        stderr: stderr_bytes,
    };
    if !output.status.success() {
        return Err(io::Error::other(format!(
            "Java exited with {}\nstdout:\n{}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        )));
    }
    Ok(output)
}

async fn read_diagnostic(
    mut reader: impl futures::AsyncRead + Unpin,
    output: &mut Vec<u8>,
) -> io::Result<()> {
    const LIMIT: usize = 64 * 1024;
    let mut buffer = vec![0; 8192];
    loop {
        let n = reader.read(&mut buffer).await?;
        if n == 0 {
            break;
        }
        let keep = n.min(LIMIT.saturating_sub(output.len()));
        output.extend_from_slice(&buffer[..keep]);
    }
    Ok(())
}

fn parse_option(output: &str, key: &str) -> Option<String> {
    output.lines().find_map(|line| {
        let (k, v) = line.split_once('=')?;
        (k.trim() == key).then(|| strip_quotes(v.trim()))
    })
}

fn strip_quotes(s: &str) -> String {
    let s = s.trim();
    if s.starts_with('"') && s.ends_with('"') && s.len() >= 2 {
        s[1..s.len() - 1].to_string()
    } else {
        s.to_string()
    }
}

fn parse_release_field(contents: &str, key: &str) -> Option<String> {
    contents.lines().find_map(|line| {
        let (k, v) = line.split_once('=')?;
        (k.trim() == key).then(|| strip_quotes(v.trim()))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SETTINGS: &str = "\
Property settings:
    java.home = /usr/lib/jvm/java-21-openjdk
    java.runtime.version = 21.0.5+11
    java.specification.version = 21
    java.vendor = Arch Linux
    java.vendor.url = https://archlinux.org/
    os.arch = amd64
";

    #[test]
    fn option_key_matches_exactly() {
        assert_eq!(
            parse_option(SETTINGS, "java.vendor").as_deref(),
            Some("Arch Linux")
        );
        assert_eq!(
            parse_option(SETTINGS, "java.vendor.url").as_deref(),
            Some("https://archlinux.org/")
        );
        assert_eq!(parse_option(SETTINGS, "os.arch").as_deref(), Some("amd64"));
        assert_eq!(parse_option(SETTINGS, "java"), None);
    }

    #[test]
    fn release_field_ignores_longer_keys() {
        let release = "JAVA_VERSION_DATE=\"2024-10-15\"\nJAVA_VERSION=\"21.0.5\"\n";

        assert_eq!(
            parse_release_field(release, "JAVA_VERSION").as_deref(),
            Some("21.0.5")
        );
    }

    #[cfg(unix)]
    #[async_std::test]
    async fn nonzero_exit_keeps_stdout_and_stderr() {
        let mut command = Command::new("sh");
        command.args(["-c", "printf stdout; printf stderr >&2; exit 7"]);
        let error = run_with_timeout(&mut command, Duration::from_secs(2))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("stdout"));
        assert!(error.to_string().contains("stderr"));
        assert!(error.to_string().contains('7'));
    }

    #[cfg(unix)]
    #[async_std::test]
    async fn timeout_terminates_process_and_keeps_partial_diagnostics() {
        let mut command = Command::new("sh");
        command.args(["-c", "printf starting >&2; exec sleep 30"]);
        let start = std::time::Instant::now();
        let error = run_with_timeout(&mut command, Duration::from_millis(100))
            .await
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert!(error.to_string().contains("starting"));
        assert!(start.elapsed() < Duration::from_secs(2));
    }

    #[cfg(unix)]
    #[async_std::test]
    async fn output_is_drained_but_capture_is_bounded() {
        let mut command = Command::new("sh");
        command.args([
            "-c",
            "head -c 100000 /dev/zero; head -c 100000 /dev/zero >&2",
        ]);
        let output = run_with_timeout(&mut command, Duration::from_secs(2))
            .await
            .unwrap();
        assert_eq!(output.stdout.len(), 64 * 1024);
        assert_eq!(output.stderr.len(), 64 * 1024);
    }

    #[cfg(unix)]
    #[async_std::test]
    async fn retained_probe_child_completion_does_not_release_home_reads_or_registration() {
        use crate::java::managed::process::JavaRetention;
        use crate::utils::{cancellation::Cancellation, process_pin::allows_cleanup};
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("java");
        let db = std::sync::Arc::new(
            crate::data::db::Database::init("sqlite::memory:")
                .await
                .unwrap(),
        );
        let java_use = JavaRetention::new(db, root.clone())
            .begin(&Cancellation::new())
            .await
            .unwrap();
        let home = root.join("fixture");
        std::fs::create_dir_all(home.join("bin")).unwrap();
        let executable = home.join("bin/java");
        std::fs::write(
            &executable,
            format!(
                "#!/bin/sh\nprintf 'java.home = {}\\njava.specification.version = 21\\n'\n",
                home.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::write(
            home.join("release"),
            "JAVA_VERSION=\"21.0.5\"\nIMPLEMENTOR=\"fixture vendor\"\n",
        )
        .unwrap();
        let found = detect_properties_retained(&executable, &java_use)
            .await
            .unwrap();
        assert_eq!(found.vendor.as_deref(), Some("fixture vendor"));
        assert_eq!(found.version.as_deref(), Some("21.0.5"));
        let scope = root.join(".managed");
        assert!(!allows_cleanup(&scope).unwrap());
        let pins: Vec<_> = std::fs::read_dir(scope.join(".sonata-processes"))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect();
        assert_eq!(pins.len(), 2);
        assert_eq!(
            pins.iter()
                .filter(|path| path.join(".sonata-exited").exists())
                .count(),
            1
        );
        java_use.finish(true);
        assert!(allows_cleanup(&scope).unwrap());
    }

    #[cfg(unix)]
    #[async_std::test]
    async fn retained_runner_finishes_success_failure_and_timeout_but_not_unknown_spawn() {
        use crate::utils::process_pin::{allows_cleanup, ProcessPin};
        for mode in ["success", "failure", "timeout", "spawn"] {
            let dir = tempfile::tempdir().unwrap();
            let pin = ProcessPin::create(dir.path()).unwrap();
            let mut command = Command::new(if mode == "spawn" {
                "/nonexistent/sonata-java"
            } else {
                "sh"
            });
            command.args([
                "-c",
                match mode {
                    "timeout" => "exec sleep 30",
                    "failure" => "exit 7",
                    _ => "exit 0",
                },
            ]);
            let result =
                run_with_timeout_retained(&mut command, Duration::from_millis(100), Some(&pin))
                    .await;
            assert_eq!(result.is_ok(), mode == "success", "{mode}");
            assert_eq!(
                allows_cleanup(dir.path()).unwrap(),
                mode != "spawn",
                "{mode}"
            );
        }
    }

    #[cfg(unix)]
    #[async_std::test]
    async fn dropped_retained_probe_keeps_child_pin_without_exit_acknowledgment() {
        use crate::utils::process_pin::{allows_cleanup, ProcessPin};
        let dir = tempfile::tempdir().unwrap();
        let ready = dir.path().join("ready");
        let pin = ProcessPin::create(dir.path()).unwrap();
        let mut command = Command::new("sh");
        command
            .args(["-c", "printf ready > \"$1\"; exec sleep 30", "sh"])
            .arg(&ready);
        let mut caller = Box::pin(run_with_timeout_retained(
            &mut command,
            Duration::from_secs(5),
            Some(&pin),
        ));
        let entered = async_std::future::timeout(Duration::from_secs(3), async {
            while !ready.exists() {
                async_std::task::sleep(Duration::from_millis(5)).await;
            }
        });
        futures::pin_mut!(entered);
        match futures::future::select(entered, caller.as_mut()).await {
            futures::future::Either::Left((result, _)) => result.unwrap(),
            futures::future::Either::Right((result, _)) => {
                panic!("probe finished early: {result:?}")
            }
        }
        drop(caller);
        assert!(!allows_cleanup(dir.path()).unwrap());
        for entry in std::fs::read_dir(dir.path().join(".sonata-processes")).unwrap() {
            let path = entry.unwrap().path();
            assert!(path.join(".sonata-starting").is_file());
            assert!(path.join(".sonata-child").is_file());
            assert!(!path.join(".sonata-exited").exists());
        }
    }

    /// Runs the `java` on `PATH`: `cargo test -- --ignored`.
    #[async_std::test]
    #[ignore]
    async fn probes_java_on_path() {
        let found = detect_properties("java").await.expect("java on PATH");

        assert!(found.major.is_some(), "{found:?}");
        assert!(found.arch.is_some(), "{found:?}");
        assert!(found.home_path.is_some(), "{found:?}");
    }
}
