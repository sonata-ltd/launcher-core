use super::*;
use std::os::unix::fs::PermissionsExt;

fn command(root: &std::path::Path, script: &str) -> ProcessorCommand {
    let java = root.join("fake-java");
    fs::write(&java, format!("#!/bin/sh\n{script}\n")).unwrap();
    fs::set_permissions(&java, fs::Permissions::from_mode(0o700)).unwrap();
    ProcessorCommand {
        java,
        main_class: "Main".into(),
        classpath: Vec::new(),
        args: Vec::new(),
        cwd: root.into(),
        log: root.join("processor.log"),
        outputs: Vec::new(),
        retention_root: root.into(),
    }
}

#[async_std::test]
async fn short_process_flushes_stdout_stderr_and_unterminated_last_line() {
    let dir = tempfile::tempdir().unwrap();
    let command = command(dir.path(), "printf 'stdout\\n'; printf 'stderr' >&2");
    let log = command.log.clone();
    let mut updates = Vec::new();
    run(command, &Cancellation::new(), |lines| updates.push(lines))
        .await
        .unwrap();
    let last = updates.last().unwrap();
    assert_eq!(
        last.iter()
            .map(|line| line.text.as_str())
            .collect::<Vec<_>>(),
        ["stdout", "stderr"]
    );
    assert_eq!(fs::read_to_string(log).unwrap(), "stdout\nstderr");
}

#[async_std::test]
async fn processor_logging_preserves_the_previous_file_when_linked() {
    let scope = tempfile::tempdir().unwrap();
    let command = command(scope.path(), "printf 'new processor log'");
    let log = command.log.clone();
    let previous = scope.path().join("previous");
    fs::write(&previous, b"preserve these bytes").unwrap();
    fs::hard_link(&previous, &log).unwrap();

    run(command, &Cancellation::new(), |_| {}).await.unwrap();

    assert_eq!(fs::read(previous).unwrap(), b"preserve these bytes");
    assert_eq!(fs::read(log).unwrap(), b"new processor log");
}

#[async_std::test]
async fn failed_process_flushes_diagnostics_before_returning_error() {
    let dir = tempfile::tempdir().unwrap();
    let command = command(dir.path(), "printf 'failure details' >&2; exit 7");
    let mut updates = Vec::new();
    let error = run(command, &Cancellation::new(), |lines| updates.push(lines))
        .await
        .unwrap_err();
    assert!(matches!(error, ModloaderError::Processor(_)));
    assert_eq!(updates.last().unwrap()[0].text, "failure details");
}

#[async_std::test]
async fn processor_log_limit_drains_output_without_blocking_child_completion() {
    let dir = tempfile::tempdir().unwrap();
    let command = command(
        dir.path(),
        "printf 'first\\n'; printf '%0102400d\\n' 0; printf 'last' >&2",
    );
    let log = command.log.clone();
    let mut updates = Vec::new();
    async_std::future::timeout(
        Duration::from_secs(10),
        run_with_log_limit(
            command,
            &Cancellation::new(),
            |lines| updates.push(lines),
            None,
            128,
        ),
    )
    .await
    .unwrap()
    .unwrap();
    let bytes = fs::read(&log).unwrap();
    assert!(bytes.len() <= 128, "{} bytes", bytes.len());
    assert!(bytes.starts_with(b"first\n"));
    assert!(updates
        .last()
        .unwrap()
        .iter()
        .any(|line| line.text.contains("log limit reached")));
    assert!(crate::utils::process_pin::allows_cleanup(dir.path()).unwrap());
}

#[async_std::test]
async fn cancellation_keeps_output_already_produced() {
    let dir = tempfile::tempdir().unwrap();
    // The shell itself stays alive: no descendant can retain the log after kill.
    let command = command(dir.path(), "printf 'ready\\n'; while :; do :; done");
    let cancel = Cancellation::new();
    let mut updates = Vec::new();
    let error = async_std::future::timeout(
        Duration::from_secs(5),
        run(command, &cancel, |lines| {
            cancel.cancel();
            updates.push(lines);
        }),
    )
    .await
    .expect("processor must observe cancellation")
    .unwrap_err();
    assert!(matches!(error, ModloaderError::Cancelled));
    assert_eq!(updates.last().unwrap()[0].text, "ready");
    assert!(crate::utils::process_pin::allows_cleanup(dir.path()).unwrap());
}

#[async_std::test]
async fn dropping_processor_future_never_acknowledges_kill_on_drop_as_exit() {
    let dir = tempfile::tempdir().unwrap();
    let command = command(dir.path(), "printf 'ready\\n'; while :; do :; done");
    let cancel = Cancellation::new();
    let (sender, receiver) = futures::channel::oneshot::channel();
    let mut sender = Some(sender);
    let mut future = Box::pin(run(command, &cancel, |_| {
        if let Some(sender) = sender.take() {
            let _ = sender.send(());
        }
    }));
    let ready = async_std::future::timeout(Duration::from_secs(5), receiver);
    futures::pin_mut!(ready);
    match futures::future::select(ready, future.as_mut()).await {
        futures::future::Either::Left((result, _)) => {
            result.unwrap().unwrap();
        }
        futures::future::Either::Right((result, _)) => {
            panic!("processor completed before drop: {result:?}")
        }
    }
    assert!(!crate::utils::process_pin::allows_cleanup(dir.path()).unwrap());
    drop(future);
    assert!(!crate::utils::process_pin::allows_cleanup(dir.path()).unwrap());
    let pins = fs::read_dir(dir.path().join(".sonata-processes")).unwrap();
    for pin in pins {
        assert!(!pin.unwrap().path().join(".sonata-exited").exists());
    }
}

#[async_std::test]
async fn failed_processor_acknowledges_confirmed_exit_not_just_success() {
    let dir = tempfile::tempdir().unwrap();
    let command = command(dir.path(), "exit 7");
    assert!(run(command, &Cancellation::new(), |_| {}).await.is_err());
    assert!(crate::utils::process_pin::allows_cleanup(dir.path()).unwrap());
}

async fn managed_use(root: &std::path::Path) -> crate::java::managed::process::JavaUse {
    let db = std::sync::Arc::new(
        crate::data::db::Database::init("sqlite::memory:")
            .await
            .unwrap(),
    );
    crate::java::managed::process::JavaRetention::new(db, root.to_owned())
        .begin(&Cancellation::new())
        .await
        .unwrap()
}

#[async_std::test]
async fn retained_processors_each_have_independent_java_pins_and_leave_outer_reads_protected() {
    use crate::utils::process_pin::allows_cleanup;
    let dir = tempfile::tempdir().unwrap();
    let java_root = dir.path().join("java");
    let java_use = managed_use(&java_root).await;
    let generation = dir.path().join("generation");
    fs::create_dir(&generation).unwrap();
    for script in ["exit 0", "exit 7"] {
        let result = run_retained(
            command(&generation, script),
            &Cancellation::new(),
            |_| {},
            Some(&java_use),
        )
        .await;
        assert_eq!(result.is_ok(), script == "exit 0");
        assert!(allows_cleanup(&generation).unwrap());
        assert!(!allows_cleanup(&java_root.join(".managed")).unwrap());
    }
    assert_eq!(
        fs::read_dir(java_root.join(".managed/.sonata-processes"))
            .unwrap()
            .count(),
        3
    );
    java_use.finish(true);
    assert!(allows_cleanup(&java_root.join(".managed")).unwrap());
}

#[async_std::test]
async fn retained_processor_cancel_confirms_exit_in_both_resources() {
    use crate::utils::process_pin::allows_cleanup;
    let dir = tempfile::tempdir().unwrap();
    let java_root = dir.path().join("java");
    let java_use = managed_use(&java_root).await;
    let cancel = Cancellation::new();
    let result = run_retained(
        command(dir.path(), "printf 'ready\\n'; while :; do :; done"),
        &cancel,
        |_| cancel.cancel(),
        Some(&java_use),
    )
    .await;
    assert!(matches!(result, Err(ModloaderError::Cancelled)));
    java_use.finish(true);
    assert!(allows_cleanup(dir.path()).unwrap());
    assert!(allows_cleanup(&java_root.join(".managed")).unwrap());
}

#[async_std::test]
async fn outer_completion_after_processor_drop_cannot_acknowledge_unknown_java_child_exit() {
    use crate::utils::process_pin::allows_cleanup;
    let dir = tempfile::tempdir().unwrap();
    let java_root = dir.path().join("java");
    let java_use = managed_use(&java_root).await;
    let cancel = Cancellation::new();
    let (entered, receiver) = futures::channel::oneshot::channel();
    let mut entered = Some(entered);
    let mut caller = Box::pin(run_retained(
        command(dir.path(), "printf 'ready\\n'; while :; do :; done"),
        &cancel,
        |_| {
            if let Some(entered) = entered.take() {
                let _ = entered.send(());
            }
        },
        Some(&java_use),
    ));
    let ready = async_std::future::timeout(Duration::from_secs(3), receiver);
    futures::pin_mut!(ready);
    match futures::future::select(ready, caller.as_mut()).await {
        futures::future::Either::Left((result, _)) => result.unwrap().unwrap(),
        futures::future::Either::Right((result, _)) => {
            panic!("processor finished early: {result:?}")
        }
    }
    drop(caller);
    java_use.finish(true);
    assert!(!allows_cleanup(dir.path()).unwrap());
    assert!(!allows_cleanup(&java_root.join(".managed")).unwrap());
}

#[async_std::test]
async fn processor_environment_is_not_inherited_and_temporary_files_stay_private() {
    let dir = tempfile::tempdir().unwrap();
    std::env::set_var("SONATA_PROCESSOR_ENVIRONMENT_PROBE", "inherited");
    let command = command(
        dir.path(),
        "printf '%s\\n%s\\n%s\\n%s\\n' \"${SONATA_PROCESSOR_ENVIRONMENT_PROBE-absent}\" \
         \"${HOME-absent}\" \"$TMPDIR\" \"$1\"",
    );
    let log = command.log.clone();
    let result = run(command, &Cancellation::new(), |_| {}).await;
    std::env::remove_var("SONATA_PROCESSOR_ENVIRONMENT_PROBE");
    result.unwrap();

    let temporary = dir.path().join(PROCESSOR_TEMP);
    assert!(temporary.is_dir());
    let observed = fs::read_to_string(log).unwrap();
    let lines: Vec<_> = observed.lines().collect();
    assert_eq!(
        lines,
        [
            "absent",
            "absent",
            temporary.to_str().unwrap(),
            &format!("-Djava.io.tmpdir={}", temporary.display()),
        ]
    );
}
