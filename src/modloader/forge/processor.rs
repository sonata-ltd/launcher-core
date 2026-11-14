use super::output::OutputTail;
use crate::data::registry::operation::message::output::OutputLine;
use crate::modloader::error::{ModloaderError, Result};
use async_std::{process::Command, task};
use sha1::{Digest, Sha1};
use std::{
    fs::{self, File},
    io::{BufReader, Read},
    path::PathBuf,
    process::Stdio,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

pub struct ProcessorCommand {
    pub java: PathBuf,
    pub main_class: String,
    pub classpath: Vec<PathBuf>,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub log: PathBuf,
    pub outputs: Vec<(PathBuf, String)>,
}

fn output_matches(path: &std::path::Path, expected: &str) -> Result<bool> {
    if expected.len() != 40 || !expected.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(ModloaderError::Invalid(
            "invalid processor output SHA-1".into(),
        ));
    }

    let file = match File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e.into()),
    };

    let mut reader = BufReader::new(file);
    let mut digest = Sha1::new();
    let mut buffer = [0u8; 64 * 1024];

    loop {
        let read = reader.read(&mut buffer)?;

        if read == 0 {
            break;
        }

        digest.update(&buffer[..read]);
    }

    Ok(format!("{:x}", digest.finalize()).eq_ignore_ascii_case(expected))
}

fn all_outputs_match(outputs: &[(PathBuf, String)]) -> Result<bool> {
    if outputs.is_empty() {
        return Ok(false);
    }

    for (path, hash) in outputs {
        if !output_matches(path, hash)? {
            return Ok(false);
        }
    }

    Ok(true)
}

pub async fn run(
    command: ProcessorCommand,
    cancel: &AtomicBool,
    mut on_output: impl FnMut(Vec<OutputLine>) + Send,
) -> Result<()> {
    if cancel.load(Ordering::Acquire) {
        return Err(ModloaderError::Cancelled);
    }

    let outputs = command.outputs.clone();

    if task::spawn_blocking(move || all_outputs_match(&outputs)).await? {
        return Ok(());
    }

    fs::create_dir_all(&command.cwd)?;

    let stdout = File::create(&command.log)?;
    let stderr = stdout.try_clone()?;

    let log_path = command.log.clone();
    let mut tail = match task::spawn_blocking(move || OutputTail::open(&log_path)).await {
        Ok(tail) => Some(tail),
        Err(error) => {
            tracing::warn!(%error, "cannot read processor output");
            None
        }
    };

    let classpath = std::env::join_paths(&command.classpath)
        .map_err(|e| ModloaderError::Invalid(e.to_string()))?;

    let mut child = Command::new(&command.java)
        .arg("-cp")
        .arg(classpath)
        .arg(&command.main_class)
        .args(&command.args)
        .current_dir(&command.cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr))
        .kill_on_drop(true)
        .spawn()?;

    let started = Instant::now();

    loop {
        let status = child.try_status();
        let finished = !matches!(&status, Ok(None));
        poll_output(&mut tail, finished, &mut on_output).await;

        if let Some(status) = status? {
            if !status.success() {
                return Err(ModloaderError::Processor(format!(
                    "exit {status}; log {}",
                    command.log.display()
                )));
            }

            break;
        }

        let cancelled = cancel.load(Ordering::Acquire);
        if cancelled || started.elapsed() > Duration::from_secs(15 * 60) {
            let killed = child.kill();
            if killed.is_ok() {
                let _ = child.status().await;
            }
            poll_output(&mut tail, true, &mut on_output).await;
            killed?;

            return Err(if cancelled {
                ModloaderError::Cancelled
            } else {
                ModloaderError::Processor("processor timed out".into())
            });
        }

        task::sleep(Duration::from_millis(100)).await;
    }

    let outputs = command.outputs;
    let has_outputs = !outputs.is_empty();
    if has_outputs && !task::spawn_blocking(move || all_outputs_match(&outputs)).await? {
        return Err(ModloaderError::Processor("output checksum mismatch".into()));
    }

    Ok(())
}

async fn poll_output(
    tail: &mut Option<OutputTail>,
    finished: bool,
    on_output: &mut (impl FnMut(Vec<OutputLine>) + Send),
) {
    let Some(mut reader) = tail.take() else {
        return;
    };
    let (reader, result) = task::spawn_blocking(move || {
        let result = reader.poll(finished);
        (reader, result)
    })
    .await;

    match result {
        Ok(snapshot) => {
            *tail = Some(reader);
            if let Some(lines) = snapshot {
                on_output(lines);
            }
        }
        Err(error) => {
            // Diagnostic preview failures must not fail an installation.
            tracing::warn!(%error, "cannot read processor output");
        }
    }
}

#[cfg(all(test, unix))]
#[path = "processor_tests.rs"]
mod tests;
