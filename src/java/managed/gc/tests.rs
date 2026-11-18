use super::*;
use crate::{
    data::db::Database,
    java::managed::{
        bytes,
        tests::{package, pin_archive, tar_fixture},
    },
    utils::{cancellation::Cancellation, object_store::ObjectStore, process_pin::ProcessPin},
};
use std::{
    sync::{Arc, Mutex, OnceLock},
    time::{SystemTime, UNIX_EPOCH},
};

const AGE: Duration = Duration::from_secs(7 * 24 * 60 * 60);
const ROOT: &str = "SONATA_TEST_JAVA_GC_ROOT";
const PHASE: &str = "SONATA_TEST_JAVA_GC_PHASE";
type Hook = (
    futures::channel::oneshot::Sender<()>,
    futures::channel::oneshot::Receiver<()>,
);
static HOOKS: OnceLock<Mutex<HashMap<(PathBuf, &'static str), Hook>>> = OnceLock::new();

pub(super) async fn checkpoint(root: &Path, phase: &'static str) {
    if std::env::var_os(ROOT).is_some_and(|configured| Path::new(&configured) == root)
        && std::env::var(PHASE).ok().as_deref() == Some(phase)
    {
        std::process::exit(76);
    }
    let hook = HOOKS
        .get_or_init(Default::default)
        .lock()
        .unwrap()
        .remove(&(root.into(), phase));
    if let Some((entered, wait)) = hook {
        let _ = entered.send(());
        let _ = wait.await;
    }
}

pub(super) fn archive_checkpoint(bytes: &Path, phase: &'static str) {
    let root = bytes.parent().unwrap().parent().unwrap();
    let phase = match phase {
        "intent" => "archive-intent",
        "unlink" => "archive-unlink",
        "alias" => "archive-alias",
        _ => panic!("unknown archive checkpoint"),
    };
    async_std::task::block_on(checkpoint(root, phase));
}

struct Fixture {
    directory: tempfile::TempDir,
    root: PathBuf,
    db: Arc<Database>,
}

async fn fixture() -> Fixture {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("java");
    let db = database(&directory.path().join("owner.db")).await;
    retention::attach(Some(db.clone()), root.clone())
        .await
        .unwrap();
    Fixture {
        directory,
        root,
        db,
    }
}

async fn database(path: &Path) -> Arc<Database> {
    Arc::new(
        Database::init(&format!("sqlite://{}", path.display()))
            .await
            .unwrap(),
    )
}

fn dead_pid() -> u32 {
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "java_gc_empty_process_fixture"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let pid = child.id();
    child.wait().unwrap();
    assert!(!process::may_be_alive(pid));
    pid
}

fn old() -> SystemTime {
    SystemTime::now() - AGE - Duration::from_secs(60)
}

fn age_files(path: &Path, time: SystemTime) {
    for entry in fs::read_dir(path).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            age_files(&path, time);
        } else {
            fs::File::options()
                .write(true)
                .open(path)
                .unwrap()
                .set_times(fs::FileTimes::new().set_modified(time))
                .unwrap();
        }
    }
}

async fn home(f: &Fixture) -> (HomeOwner, PathBuf) {
    let source = f.directory.path().join("fixture.tar.gz");
    tar_fixture(&source);
    let package = pin_archive(package(), &source);
    let archive = bytes::import(&f.root, source, &package, &Cancellation::new())
        .await
        .unwrap();
    let archive_path = archive.path.clone();
    drop(archive);
    let mut owner = HomeOwner::new(
        Uuid::new_v4(),
        &package,
        "jre".into(),
        Some(f.db.retention_owner().await.unwrap().unwrap().identity),
    )
    .unwrap();
    owner.pid = dead_pid();
    owner.created = old().duration_since(UNIX_EPOCH).unwrap().as_secs();
    let path = f.root.join(owner.generation());
    fs::create_dir_all(path.join("jre/bin")).unwrap();
    fs::write(path.join("jre/bin/java"), b"immutable extracted fixture").unwrap();
    owner.write(&path).unwrap();
    age_files(&path, old());
    fs::File::options()
        .write(true)
        .open(&archive_path)
        .unwrap()
        .set_times(fs::FileTimes::new().set_modified(old()))
        .unwrap();
    (owner, archive_path)
}

async fn register(db: &Database, owner: &HomeOwner) {
    let home = owner.generation().join(&owner.home);
    sqlx::query(
        "INSERT INTO java_runtimes(version,exec_path,home_path,source) VALUES('21',?,?,'managed')",
    )
    .bind(home.join("bin/java").to_str().unwrap())
    .bind(home.to_str().unwrap())
    .execute(&db.pool)
    .await
    .unwrap();
}

async fn crash(f: &Fixture, phase: &str) {
    let root = f.root.clone();
    let phase = phase.to_owned();
    let output = async_std::task::spawn_blocking(move || {
        std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "java::managed::gc::tests::crash_collector",
                "--nocapture",
            ])
            .env(ROOT, root)
            .env(PHASE, phase)
            .output()
            .unwrap()
    })
    .await;
    assert_eq!(
        output.status.code(),
        Some(76),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[async_std::test]
async fn crash_collector() {
    let Some(root) = std::env::var_os(ROOT).map(PathBuf::from) else {
        return;
    };
    collect(root, AGE).await.unwrap();
    panic!("configured GC boundary was not reached");
}

#[async_std::test]
async fn dead_owned_unregistered_home_and_source_archive_are_collected_without_sql_identity_changes(
) {
    let f = fixture().await;
    let (owner, archive) = home(&f).await;
    let identity = f.db.retention_owner().await.unwrap().unwrap();
    assert_eq!(collect(f.root.clone(), AGE).await.unwrap(), (1, 1));
    assert!(!f.root.join(owner.generation()).exists());
    assert!(!archive.exists());
    assert_eq!(f.db.retention_owner().await.unwrap().unwrap(), identity);
    assert!(fs::read_dir(f.root.join(QUEUE)).unwrap().next().is_none());
}

#[async_std::test]
async fn every_owner_registration_and_operation_home_retains_home_and_source_archive() {
    for mode in ["second-db", "operation"] {
        let f = fixture().await;
        let second = database(&f.directory.path().join("second.db")).await;
        retention::attach(Some(second.clone()), f.root.clone())
            .await
            .unwrap();
        let (owner, archive) = home(&f).await;
        if mode == "second-db" {
            register(&second, &owner).await;
        } else {
            let record = super::super::Record {
                snapshot: super::super::ManagedJavaSnapshot {
                    id: owner.operation,
                    created_at: chrono::Utc::now(),
                    package: pin_archive(package(), &f.directory.path().join("fixture.tar.gz")),
                    stage: super::super::ManagedJavaStage::AwaitingValidation,
                    downloaded_bytes: 0,
                    total_bytes: Some(owner.size),
                    runtime_id: None,
                    error: None,
                },
                home: Some(owner.generation().join(&owner.home)),
                owner: Some(super::super::ownership::Owner::new(owner.operation)),
            };
            super::super::write_record(&f.root, &record).unwrap();
        }
        assert_eq!(
            collect(f.root.clone(), AGE).await.unwrap(),
            (0, 0),
            "{mode}"
        );
        assert!(f.root.join(owner.generation()).exists());
        assert!(archive.exists());
    }
}

#[async_std::test]
async fn live_publisher_recent_home_or_output_keeps_source_archive_without_sql_roots() {
    for mode in ["pid", "created", "output"] {
        let f = fixture().await;
        let (mut owner, archive) = home(&f).await;
        let path = f.root.join(owner.generation());
        match mode {
            "pid" => owner.pid = std::process::id(),
            "created" => {
                owner.created = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_secs()
            }
            _ => fs::write(path.join("jre/bin/java"), b"new output").unwrap(),
        }
        if mode != "output" {
            fs::write(path.join(HOME_MARKER), serde_json::to_vec(&owner).unwrap()).unwrap();
            age_files(&path, old());
        }
        assert_eq!(
            collect(f.root.clone(), AGE).await.unwrap(),
            (0, 0),
            "{mode}"
        );
        assert!(path.exists());
        assert!(archive.exists());
    }
}

#[async_std::test]
async fn managed_admission_archive_admission_and_dropped_broad_process_pin_block_collection() {
    for mode in ["managed", "archive", "pin"] {
        let f = fixture().await;
        let (owner, archive) = home(&f).await;
        let access = match mode {
            "managed" => Some(
                ObjectStore::new(f.root.join(".managed"))
                    .try_access()
                    .unwrap(),
            ),
            "archive" => Some(
                ObjectStore::new(f.root.join(".managed/bytes"))
                    .try_access()
                    .unwrap(),
            ),
            _ => {
                drop(ProcessPin::create(&f.root.join(".managed")).unwrap());
                None
            }
        };
        assert!(collect(f.root.clone(), AGE).await.is_err(), "{mode}");
        assert!(f.root.join(owner.generation()).exists());
        assert!(archive.exists());
        drop(access);
    }
}

#[async_std::test]
async fn unknown_and_future_metadata_layouts_or_scratch_defer_both_home_and_archive_collection() {
    for mode in [
        "root",
        "metadata",
        "scratch",
        "marker",
        "future-home",
        "future-bytes",
        "alias",
        "intent",
        "quarantine",
    ] {
        let f = fixture().await;
        let (owner, archive) = home(&f).await;
        let generation = f.root.join(owner.generation());
        match mode {
            "root" => {
                fs::write(f.root.join("unknown"), b"keep").unwrap();
            }
            "metadata" => {
                fs::write(f.root.join(".managed/unknown"), b"keep").unwrap();
            }
            "scratch" => {
                fs::create_dir_all(f.root.join(".managed/staging/unknown")).unwrap();
            }
            "marker" => {
                fs::remove_file(generation.join(HOME_MARKER)).unwrap();
            }
            "future-home" => {
                let mut json = serde_json::to_value(&owner).unwrap();
                json["created"] = u64::MAX.into();
                fs::write(
                    generation.join(HOME_MARKER),
                    serde_json::to_vec(&json).unwrap(),
                )
                .unwrap();
            }
            "future-bytes" => {
                fs::File::options()
                    .write(true)
                    .open(&archive)
                    .unwrap()
                    .set_times(fs::FileTimes::new().set_modified(SystemTime::now() + AGE))
                    .unwrap();
            }
            "alias" => {
                fs::write(
                    f.root
                        .join(".managed/bytes/aliases/sha1")
                        .join("ab".repeat(20)),
                    b"broken",
                )
                .unwrap();
            }
            "intent" => {
                fs::create_dir_all(f.root.join(".managed/bytes/.sonata-gc")).unwrap();
                fs::write(
                    f.root.join(".managed/bytes/.sonata-gc/broken.json"),
                    b"broken",
                )
                .unwrap();
            }
            _ => {
                fs::create_dir_all(f.root.join(QUEUE).join(Uuid::new_v4().to_string())).unwrap();
            }
        }
        assert!(collect(f.root.clone(), AGE).await.is_err(), "{mode}");
        assert!(generation.join("jre/bin/java").exists());
        assert!(archive.exists());
    }
}

#[async_std::test]
async fn missing_historical_database_defers_collection_without_recreating_database() {
    let f = fixture().await;
    let (owner, archive) = home(&f).await;
    f.db.pool.close().await;
    let path = f.directory.path().join("owner.db");
    fs::remove_file(&path).unwrap();
    assert!(collect(f.root.clone(), AGE).await.is_err());
    assert!(!path.exists());
    assert!(f.root.join(owner.generation()).exists());
    assert!(archive.exists());
}

#[async_std::test]
async fn process_exit_replays_every_home_and_archive_deletion_boundary() {
    for phase in [
        "home-intent",
        "home-rename",
        "home-outputs",
        "home-metadata",
        "home-removed",
        "home-cleaned",
        "archive-intent",
        "archive-unlink",
        "archive-alias",
        "archive-cleaned",
    ] {
        let f = fixture().await;
        let (owner, archive) = home(&f).await;
        crash(&f, phase).await;
        collect(f.root.clone(), AGE).await.unwrap();
        assert!(!f.root.join(owner.generation()).exists(), "{phase}");
        assert!(!archive.exists(), "{phase}");
        assert!(
            fs::read_dir(f.root.join(QUEUE)).unwrap().next().is_none(),
            "{phase}"
        );
        assert!(
            fs::read_dir(f.root.join(".managed/bytes/.sonata-gc"))
                .unwrap()
                .next()
                .is_none(),
            "{phase}"
        );
    }
}

#[async_std::test]
async fn replay_restores_newly_registered_quarantined_home_and_retains_archive() {
    let f = fixture().await;
    let (owner, archive) = home(&f).await;
    let path = f.root.join(owner.generation());
    crash(&f, "home-rename").await;
    assert!(!path.exists());
    register(&f.db, &owner).await;
    assert_eq!(collect(f.root.clone(), AGE).await.unwrap(), (0, 0));
    assert_eq!(
        fs::read(path.join("jre/bin/java")).unwrap(),
        b"immutable extracted fixture"
    );
    assert!(archive.exists());
}

#[async_std::test]
async fn replay_rechecks_new_output_age_and_blocks_archives_while_home_quarantine_is_pending() {
    let f = fixture().await;
    let (owner, archive) = home(&f).await;
    crash(&f, "home-rename").await;
    let quarantine = f.root.join(QUEUE).join(owner.operation.to_string());
    fs::write(quarantine.join("jre/bin/java"), b"new output").unwrap();
    assert!(collect(f.root.clone(), AGE).await.is_err());
    assert!(quarantine.exists());
    assert!(archive.exists());
    age_files(&quarantine, old());
    assert_eq!(collect(f.root.clone(), AGE).await.unwrap(), (1, 1));
}

#[async_std::test]
async fn replay_reimported_archive_age_and_new_operation_roots_prevent_deletion() {
    for mode in ["age", "root"] {
        let f = fixture().await;
        let (_owner, archive) = home(&f).await;
        crash(&f, "archive-intent").await;
        if mode == "age" {
            fs::remove_file(&archive).unwrap();
            let source = f.directory.path().join("fixture.tar.gz");
            let package = pin_archive(package(), &source);
            drop(
                bytes::import(&f.root, source, &package, &Cancellation::new())
                    .await
                    .unwrap(),
            );
        } else {
            let mut package = pin_archive(package(), &f.directory.path().join("fixture.tar.gz"));
            package.id = "retaining-package".into();
            let id = Uuid::new_v4();
            let record = super::super::Record {
                snapshot: super::super::ManagedJavaSnapshot {
                    id,
                    created_at: chrono::Utc::now(),
                    package,
                    stage: super::super::ManagedJavaStage::Cancelled,
                    downloaded_bytes: 0,
                    total_bytes: None,
                    runtime_id: None,
                    error: None,
                },
                home: None,
                owner: Some(super::super::ownership::Owner::new(id)),
            };
            super::super::write_record(&f.root, &record).unwrap();
        }
        assert_eq!(
            collect(f.root.clone(), AGE).await.unwrap(),
            (0, 0),
            "{mode}"
        );
        assert!(archive.exists());
    }
}

#[async_std::test]
async fn dropped_collector_keeps_all_database_locks_and_both_admissions_until_owned_cleanup() {
    for phase in ["snapshot", "home-intent"] {
        let f = fixture().await;
        let (owner, archive) = home(&f).await;
        let (entered, ready) = futures::channel::oneshot::channel();
        let (resume, wait) = futures::channel::oneshot::channel();
        let (completed, done) = futures::channel::oneshot::channel();
        let (finish, wait_finished) = futures::channel::oneshot::channel();
        {
            let mut hooks = HOOKS.get_or_init(Default::default).lock().unwrap();
            hooks.insert((f.root.clone(), phase), (entered, wait));
            hooks.insert((f.root.clone(), "collected"), (completed, wait_finished));
        }
        let mut caller = Box::pin(collect(f.root.clone(), AGE));
        match futures::future::select(ready, caller.as_mut()).await {
            futures::future::Either::Left((result, _)) => result.unwrap(),
            futures::future::Either::Right((result, _)) => {
                panic!("collector finished early: {result:?}")
            }
        }
        drop(caller);
        for scope in [".managed", ".managed/bytes"] {
            assert!(ObjectStore::new(f.root.join(scope)).try_access().is_err());
        }
        let options: sqlx::sqlite::SqliteConnectOptions =
            format!("sqlite://{}", f.directory.path().join("owner.db").display())
                .parse()
                .unwrap();
        let probe = sqlx::SqlitePool::connect_with(options.busy_timeout(Duration::from_millis(20)))
            .await
            .unwrap();
        assert!(sqlx::query("UPDATE java_runtimes SET vendor='probe'")
            .execute(&probe)
            .await
            .is_err());
        resume.send(()).unwrap();
        done.await.unwrap();
        assert!(!f.root.join(owner.generation()).exists());
        assert!(!archive.exists());
        sqlx::query("UPDATE java_runtimes SET vendor='probe'")
            .execute(&probe)
            .await
            .unwrap();
        probe.close().await;
        finish.send(()).unwrap();
    }
}

#[cfg(unix)]
#[async_std::test]
async fn aliases_share_collector_gate_and_linked_outputs_or_quarantine_preserve_external_data() {
    for mode in ["alias", "output", "quarantine"] {
        let f = fixture().await;
        let (owner, archive) = home(&f).await;
        let outside = f.directory.path().join("outside");
        fs::write(&outside, b"external bytes").unwrap();
        if mode == "alias" {
            let alias = f.directory.path().join("alias");
            std::os::unix::fs::symlink(&f.root, &alias).unwrap();
            assert_eq!(collect(alias, AGE).await.unwrap(), (1, 1));
        } else if mode == "output" {
            std::os::unix::fs::symlink(
                &outside,
                f.root.join(owner.generation()).join("jre/linked"),
            )
            .unwrap();
            assert!(collect(f.root.clone(), AGE).await.is_err());
            assert!(archive.exists());
        } else {
            crash(&f, "home-rename").await;
            let quarantine = f.root.join(QUEUE).join(owner.operation.to_string());
            fs::rename(&quarantine, f.directory.path().join("kept-quarantine")).unwrap();
            std::os::unix::fs::symlink(&outside, quarantine).unwrap();
            assert!(collect(f.root.clone(), AGE).await.is_err());
            assert!(archive.exists());
        }
        assert_eq!(fs::read(outside).unwrap(), b"external bytes");
    }
}

#[async_std::test]
async fn replay_preserves_replacement_home_unknown_partial_quarantine_and_new_process_pin() {
    for mode in ["replacement", "partial", "process"] {
        let f = fixture().await;
        let (owner, archive) = home(&f).await;
        crash(
            &f,
            if mode == "partial" {
                "home-metadata"
            } else {
                "home-rename"
            },
        )
        .await;
        let quarantine = f.root.join(QUEUE).join(owner.operation.to_string());
        match mode {
            "replacement" => {
                let original = f.root.join(owner.generation());
                fs::create_dir_all(original.join("jre/bin")).unwrap();
                fs::write(original.join("jre/bin/java"), b"replacement home").unwrap();
                let mut replacement = owner.clone();
                replacement.pid = std::process::id();
                replacement.write(&original).unwrap();
            }
            "partial" => {
                fs::write(quarantine.join("unknown-output"), b"keep").unwrap();
            }
            _ => {
                drop(ProcessPin::create(&f.root.join(".managed")).unwrap());
            }
        }
        assert!(collect(f.root.clone(), AGE).await.is_err(), "{mode}");
        assert!(archive.exists());
        assert!(quarantine.exists());
        if mode == "replacement" {
            assert_eq!(
                fs::read(f.root.join(owner.generation()).join("jre/bin/java")).unwrap(),
                b"replacement home"
            );
        }
        if mode == "partial" {
            assert_eq!(
                fs::read(quarantine.join("unknown-output")).unwrap(),
                b"keep"
            );
        }
    }
}

#[async_std::test]
async fn startup_collects_only_eligible_orphans_and_preserves_registered_home_without_discovery() {
    let f = fixture().await;
    let (retained, archive) = home(&f).await;
    register(&f.db, &retained).await;
    let (orphan, _) = home(&f).await;
    let service = crate::java::service::JavaService::new(
        f.db.clone(),
        crate::bus::EventBus::new(16),
        f.root.clone(),
    );
    service.initialize().await.unwrap();
    assert!(f.root.join(retained.generation()).exists());
    assert!(!f.root.join(orphan.generation()).exists());
    assert!(archive.exists());
    assert_eq!(service.list_all().await.unwrap().len(), 1);
}

#[async_std::test]
async fn authoritative_orphan_archive_collection_ignores_alias_roots_and_preserves_corrupt_bytes() {
    for corrupt in [false, true] {
        let f = fixture().await;
        let (owner, archive) = home(&f).await;
        fs::remove_dir_all(f.root.join(owner.generation())).unwrap();
        if corrupt {
            fs::write(&archive, b"corrupt canonical bytes").unwrap();
            fs::File::options()
                .write(true)
                .open(&archive)
                .unwrap()
                .set_times(fs::FileTimes::new().set_modified(old()))
                .unwrap();
        }
        assert_eq!(
            collect(f.root.clone(), AGE).await.unwrap(),
            (0, usize::from(!corrupt))
        );
        if corrupt {
            assert_eq!(fs::read(archive).unwrap(), b"corrupt canonical bytes");
        } else {
            assert!(!archive.exists());
        }
    }
}
