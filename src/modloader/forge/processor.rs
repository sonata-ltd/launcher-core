use super::output::{OutputCapture, OutputTail};
use crate::data::registry::operation::message::output::OutputLine;
use crate::modloader::error::{ModloaderError, Result};
use crate::utils::{cancellation::Cancellation, files};
use async_std::{process::Command, task};
use std::{
    fs,
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
    /// Entire generation, not just the extracted processor work directory.
    pub retention_root: PathBuf,
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

#[cfg(test)]
pub async fn run(
    command: ProcessorCommand,
    cancel: &Cancellation,
    on_output: impl FnMut(Vec<OutputLine>) + Send,
) -> Result<()> {
    run_retained(command, cancel, on_output, None).await
}

pub(crate) async fn run_retained(
    command: ProcessorCommand,
    cancel: &Cancellation,
    on_output: impl FnMut(Vec<OutputLine>) + Send,
    java_use: Option<&crate::java::managed::process::JavaUse>,
) -> Result<()> {
    run_with_log_limit(command, cancel, on_output, java_use, 32 * 1024 * 1024).await
}

async fn run_with_log_limit(
    command: ProcessorCommand,
    cancel: &Cancellation,
    mut on_output: impl FnMut(Vec<OutputLine>) + Send,
    java_use: Option<&crate::java::managed::process::JavaUse>,
    log_limit: u64,
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

    let (mut capture, stdout, stderr) = OutputCapture::open(&command.log, log_limit)?;

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

    let pin = crate::utils::process_pin::ProcessPin::create(&command.retention_root)?;
    let java_pin = match java_use.map(|java_use| java_use.child_pin()).transpose() {
        Ok(pin) => pin,
        Err(error) => {
            pin.abort_before_spawn();
            return Err(error.into());
        }
    };
    let pins: Vec<_> = std::iter::once(&pin).chain(java_pin.iter()).collect();
    crate::utils::process_pin::prepare_spawn(std::path::Path::new(&command.java), &pins)?;
    let child = Command::new(&command.java)
        .arg("-cp")
        .arg(classpath)
        .arg(&command.main_class)
        .args(&command.args)
        .current_dir(&command.cwd)
        .stdin(Stdio::null())
        .stdout(stdout)
        .stderr(stderr)
        .kill_on_drop(true)
        .spawn()?;
    pin.record_child(child.id());
    if let Some(pin) = &java_pin {
        pin.record_child(child.id());
    }
    let mut process_exited = false;
    let result = run_child(
        command,
        child,
        cancel,
        &mut on_output,
        &mut tail,
        &mut capture,
        &mut process_exited,
    )
    .await;
    pin.finish(process_exited);
    if let Some(pin) = &java_pin {
        pin.finish(process_exited);
    }
    result
}

async fn run_child(
    command: ProcessorCommand,
    mut child: async_std::process::Child,
    cancel: &Cancellation,
    on_output: &mut (impl FnMut(Vec<OutputLine>) + Send),
    tail: &mut Option<OutputTail>,
    capture: &mut OutputCapture,
    process_exited: &mut bool,
) -> Result<()> {
    let started = Instant::now();

    loop {
        let status = child.try_status();
        let finished = !matches!(&status, Ok(None));
        *process_exited = matches!(&status, Ok(Some(_)));
        let drained = match capture.drain(if finished { 1024 * 1024 } else { 64 * 1024 }) {
            Ok(bytes) => bytes,
            Err(error) => {
                if !*process_exited && child.kill().is_ok() {
                    *process_exited = child.status().await.is_ok();
                }
                return Err(error.into());
            }
        };
        poll_output(tail, finished, on_output).await;

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
                *process_exited = child.status().await.is_ok();
            }
            if let Err(error) = capture.drain(1024 * 1024) {
                tracing::warn!(%error, "could not drain processor output after stop");
            }
            poll_output(tail, true, on_output).await;
            killed?;

            return Err(if cancelled {
                ModloaderError::Cancelled
            } else {
                ModloaderError::Processor("processor timed out".into())
            });
        }

        if drained > 0 {
            task::yield_now().await;
        } else {
            let _ = cancel.run(task::sleep(Duration::from_millis(10))).await;
        }
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
