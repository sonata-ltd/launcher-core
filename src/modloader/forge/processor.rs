use super::output::OutputTail;
use crate::data::registry::operation::message::output::OutputLine;
use crate::modloader::error::{ModloaderError, Result};
use crate::utils::{cancellation::Cancellation, files};
use async_std::{process::Command, task};
use std::{
    fs::{self, File},
    path::PathBuf,
    process::Stdio,
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

fn output_matches(path: &std::path::Path, expected: &str, cancel: &Cancellation) -> Result<bool> {
    if !files::hash_valid(expected, 40) {
        return Err(ModloaderError::Invalid(
            "invalid processor output SHA-1".into(),
        ));
    }

    let matches = files::matches_cancellable(path, expected, cancel);
    if cancel.is_cancelled() {
        return Err(ModloaderError::Cancelled);
    }
    Ok(matches?)
}

fn all_outputs_match(outputs: &[(PathBuf, String)], cancel: &Cancellation) -> Result<bool> {
    if outputs.is_empty() {
        return Ok(false);
    }

    for (path, hash) in outputs {
        if !output_matches(path, hash, cancel)? {
            return Ok(false);
        }
    }

    Ok(true)
}

pub async fn run(
    command: ProcessorCommand,
    cancel: &Cancellation,
    mut on_output: impl FnMut(Vec<OutputLine>) + Send,
) -> Result<()> {
    if cancel.is_cancelled() {
        return Err(ModloaderError::Cancelled);
    }

    let outputs = command.outputs.clone();

    let token = cancel.clone();
    if task::spawn_blocking(move || all_outputs_match(&outputs, &token)).await? {
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

        let cancelled = cancel.is_cancelled();
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

        let _ = cancel.run(task::sleep(Duration::from_millis(100))).await;
    }

    let outputs = command.outputs;
    let has_outputs = !outputs.is_empty();
    let token = cancel.clone();
    if has_outputs && !task::spawn_blocking(move || all_outputs_match(&outputs, &token)).await? {
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
