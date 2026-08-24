use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use async_std::{fs, future, io, process::Command};
use serde::Serialize;

#[derive(Serialize)]
pub struct DetectedJavaRuntime {
    version: Option<String>,
    exec_path: PathBuf,
    home_path: Option<PathBuf>,
    vendor: Option<String>,
}

pub async fn detect_properties(input: impl AsRef<Path>) -> io::Result<DetectedJavaRuntime> {
    let input = input.as_ref();

    let exec_path = match input.canonicalize() {
        Ok(p) => p,
        Err(_) => input.to_path_buf(),
    };

    let output = run_with_timeout(
        Command::new(&exec_path).args(["-XshowSettings:properties", "-version"]),
        Duration::from_secs(5),
    )
    .await?;

    // java -version writes only to stderr but we still have to check the stdout
    let output = {
        let mut s = String::new();
        s.push_str(&String::from_utf8_lossy(&output.stdout));
        s.push('\n');
        s.push_str(&String::from_utf8_lossy(&output.stderr));
        s
    };

    let home_path = parse_option(&output, "java.home").map(PathBuf::from);
    let mut version = parse_option(&output, "java.runtime.version");
    let mut vendor = parse_option(&output, "java.vendor");

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

    Ok(DetectedJavaRuntime {
        version,
        exec_path,
        home_path,
        vendor,
    })
}

async fn run_with_timeout(cmd: &mut Command, dur: Duration) -> io::Result<std::process::Output> {
    let fut = cmd.output();
    match future::timeout(dur, fut).await {
        Ok(Ok(out)) => Ok(out),
        Ok(Err(e)) => Err(e),
        Err(_) => Err(io::Error::new(io::ErrorKind::TimedOut, "process timed out")),
    }
}

fn parse_option(output: &str, key: &str) -> Option<String> {
    for line in output.lines() {
        let line = line.trim();

        if line.starts_with(key) {
            if let Some(eq_pos) = line.find('=') {
                let val = line[eq_pos + 1..].trim();
                return Some(strip_quotes(val));
            }
        } else {
            let parts: Vec<&str> = line.splitn(2, '=').collect();
            if parts.len() == 2 && parts[0].trim() == key {
                return Some(strip_quotes(parts[1].trim()));
            }
        }
    }

    None
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
    for line in contents.lines() {
        let line = line.trim();

        if line.starts_with(key) {
            if let Some(eq) = line.find('=') {
                let val = line[eq + 1..].trim();
                return Some(strip_quotes(val));
            }
        }
    }

    None
}
