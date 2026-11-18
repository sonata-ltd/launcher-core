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
