use super::output::{OutputCapture, OutputTail};
use crate::data::registry::operation::message::output::OutputLine;
use crate::modloader::error::{ModloaderError, Result};
use crate::utils::process_confinement::{self, Confinement};
use crate::utils::process_group::ProcessGroup;
use crate::utils::{
    cancellation::Cancellation,
    files::{self, directory::Directory},
};
use async_std::{process::Command, task};
#[cfg(all(test, unix))]
use std::fs;
use std::{
    path::PathBuf,
    process::Stdio,
    time::{Duration, Instant},
};

pub struct ProcessorCommand {
    pub java: PathBuf,
    pub main_class: String,
    pub classpath: Vec<PathBuf>,
    pub args: Vec<String>,
    pub cwd: std::sync::Arc<Directory>,
    pub log: PathBuf,
    pub outputs: Vec<(PathBuf, String)>,
    /// Entire generation, not just the extracted processor work directory.
    pub retention_root: std::sync::Arc<Directory>,
}

/// Private temporary directory created inside the processor work directory.
const PROCESSOR_TEMP: &str = "tmp";

/// Variables a JVM needs to start and to decode non-ASCII paths. Everything
/// else, including JVM option injection variables, is not inherited.
#[cfg(windows)]
const INHERITED_ENVIRONMENT: &[&str] = &["SystemRoot", "SystemDrive", "windir"];
#[cfg(not(windows))]
const INHERITED_ENVIRONMENT: &[&str] = &["LANG", "LC_ALL", "LC_CTYPE"];

fn confine_environment(command: &mut std::process::Command, temporary: &std::path::Path) {
    command.env_clear();
    for name in INHERITED_ENVIRONMENT {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    for name in ["TMPDIR", "TMP", "TEMP"] {
        command.env(name, temporary);
    }
}

fn java_temporary_property(temporary: &std::path::Path) -> std::ffi::OsString {
    let mut property = std::ffi::OsString::from("-Djava.io.tmpdir=");
    property.push(temporary);
    property
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

    let cwd = command.cwd.clone();
    let log = command.log.strip_prefix(cwd.path()).map_err(|_| {
        ModloaderError::Invalid("processor log is outside the work directory".into())
    })?;
    let log_directory = cwd.create_entry_parent(log)?;
    let log_name = log
        .file_name()
        .ok_or_else(|| ModloaderError::Invalid("processor log has no name".into()))?
        .to_owned();

    let (mut capture, stdout, stderr) =
        OutputCapture::open_in(&log_directory, &log_name, log_limit)?;

    let mut tail = match task::spawn_blocking(move || {
        log_directory
            .open_file(&log_name)
            .map(OutputTail::from_file)
    })
    .await
    {
        Ok(tail) => Some(tail),
        Err(error) => {
            tracing::warn!(%error, "cannot read processor output");
            None
        }
    };

    let classpath = std::env::join_paths(&command.classpath)
        .map_err(|e| ModloaderError::Invalid(e.to_string()))?;

    let temporary = cwd.create_relative(std::path::Path::new(PROCESSOR_TEMP))?;
    // Writes of the processor and its descendants stay inside the generation.
    let confinement = Confinement::writable_beneath(&command.retention_root).map_err(|error| {
        if process_confinement::is_unavailable(&error) {
            ModloaderError::ProcessorConfinementUnavailable(error.to_string())
        } else {
            error.into()
        }
    })?;
    let pin = crate::utils::process_pin::ProcessPin::create_in(&command.retention_root)?;
    let java_pin = match java_use.map(|java_use| java_use.child_pin()).transpose() {
        Ok(pin) => pin,
        Err(error) => {
            pin.abort_before_spawn();
            return Err(error.into());
        }
    };
    let pins: Vec<_> = std::iter::once(&pin).chain(java_pin.iter()).collect();
    crate::utils::process_pin::prepare_spawn(std::path::Path::new(&command.java), &pins)?;
    let mut process = std::process::Command::new(&command.java);
    confine_environment(&mut process, temporary.path());
    process
        .arg(java_temporary_property(temporary.path()))
        .arg("-cp")
        .arg(classpath)
        .arg(&command.main_class)
        .args(&command.args)
        .current_dir(cwd.path());
    confinement.apply(&mut process);
    ProcessGroup::isolate(&mut process);
    let child = Command::from(process)
        .stdin(Stdio::null())
        .stdout(stdout)
        .stderr(stderr)
        .kill_on_drop(true)
        .spawn()?;
    drop(confinement);
    let mut group = ProcessGroup::of_leader(child.id());
    pin.record_child(child.id());
    if let Some(pin) = &java_pin {
        pin.record_child(child.id());
    }
    let outputs = command.outputs.clone();
    let mut process_exited = false;
    let result = run_child(
        command,
        child,
        &group,
        cancel,
        &mut on_output,
        &mut tail,
        &mut capture,
        &mut process_exited,
    )
    .await;
    // Descendants must be gone before outputs are trusted or pins released.
    let descendants_gone = group.terminate(DESCENDANT_EXIT_TIMEOUT).await;
    let result = match result {
        Ok(()) if !descendants_gone => Err(ModloaderError::Processor(
            "processor descendants did not exit".into(),
        )),
        Ok(()) if !outputs.is_empty() => {
            let token = cancel.clone();
            if task::spawn_blocking(move || all_outputs_match(&outputs, &token)).await? {
                Ok(())
            } else {
                Err(ModloaderError::Processor("output checksum mismatch".into()))
            }
        }
        result => result,
    };
    let acknowledged = process_exited && descendants_gone;
    pin.finish(acknowledged);
    if let Some(pin) = &java_pin {
        pin.finish(acknowledged);
    }
    result
}

/// How long the process group may take to empty after SIGKILL.
const DESCENDANT_EXIT_TIMEOUT: Duration = Duration::from_secs(5);

async fn run_child(
    command: ProcessorCommand,
    mut child: async_std::process::Child,
    group: &ProcessGroup,
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
                if !*process_exited {
                    group.kill();
                }
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
            group.kill();
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
