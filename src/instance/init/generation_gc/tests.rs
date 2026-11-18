use super::*;
use crate::{
    data::{config::Config, db::Database},
    instance::init::{
        model::{InstalledArtifact, InstalledProfile},
        storage,
        workspace::InstallWorkspace,
    },
    java::policy::JavaPolicy,
    modloader::model::LoaderSpec,
    utils::cancellation::Cancellation,
};
use sha1::Digest;

struct Fixture {
    directory: tempfile::TempDir,
    paths: Arc<LauncherPaths>,
    db: Arc<Database>,
}

async fn paths(root: &Path) -> Arc<LauncherPaths> {
    Arc::new(
        LauncherPaths::resolve(&Config {
            root_override: Some(root.to_owned()),
            ..Config::init()
        })
        .await
        .unwrap(),
    )
}

async fn fixture() -> Fixture {
    let directory = tempfile::tempdir().unwrap();
    let paths = paths(directory.path()).await;
    let db = Arc::new(
        Database::init(&format!(
            "sqlite://{}",
            directory.path().join("owner.db").display()
        ))
        .await
        .unwrap(),
    );
    sqlx::query("INSERT INTO instances(id,name,dir,version,loader,manifest_url,meta_provider) VALUES(1,'generation','generation','1.21','vanilla','https://example.invalid','prism')").execute(&db.pool).await.unwrap();
    retention::attach(db.clone(), paths.libraries().clone(), Cancellation::new())
        .await
        .unwrap();
    Fixture {
        directory,
        paths,
        db,
    }
}

const ROOT: &str = "SONATA_TEST_GENERATION_ROOT";
const PHASE: &str = "SONATA_TEST_GENERATION_PHASE";

#[test]
fn generation_writer() {
    let Some(root) = std::env::var_os(ROOT).map(PathBuf::from) else {
        return;
    };
    if std::env::var(PHASE).ok().as_deref() != Some("writer") {
        return;
    }
    async_std::task::block_on(async {
        let paths = paths(&root).await;
        let db = Arc::new(
            Database::init(&format!("sqlite://{}", root.join("owner.db").display()))
                .await
                .unwrap(),
        );
        retention::attach(db.clone(), paths.libraries().clone(), Cancellation::new())
            .await
            .unwrap();
        let workspace = InstallWorkspace::create_owned(&paths, 1.into(), &Cancellation::new(), &db)
            .await
            .unwrap();
        fs::write(workspace.maven_dir().join("library.jar"), b"runtime bytes").unwrap();
        fs::write(workspace.work_dir().join("installer.log"), b"diagnostics").unwrap();
        fs::write(
            root.join("last-generation"),
            workspace
                .relative_maven_dir()
                .parent()
                .unwrap()
                .to_str()
                .unwrap(),
        )
        .unwrap();
        std::process::exit(76);
    });
}

async fn child(f: &Fixture, test: &str, phase: &str, status: i32) {
    let root = f.directory.path().to_owned();
    let test = test.to_owned();
    let phase = phase.to_owned();
    let output = async_std::task::spawn_blocking(move || {
        std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                &format!("instance::init::generation_gc::tests::{test}"),
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
        Some(status),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

async fn generation(f: &Fixture) -> PathBuf {
    child(f, "generation_writer", "writer", 76).await;
    let path =
        PathBuf::from(fs::read_to_string(f.directory.path().join("last-generation")).unwrap());
    assert!(!process::may_be_alive(
        record(&f.paths.libraries().join(&path)).unwrap().pid
    ));
    path
}

fn current_profile(relative: &Path) -> InstalledProfile {
    InstalledProfile {
        format_version: 1,
        minecraft: "1.21".into(),
        loader: LoaderSpec::vanilla(),
        manifest: serde_json::json!({}),
        java: JavaPolicy {
            majors: vec![21],
            allow_newer: false,
        },
        libraries_root: relative.join("maven"),
        artifacts: vec![InstalledArtifact {
            path: "library.jar".into(),
            sha1: format!("{:x}", sha1::Sha1::digest(b"runtime bytes")),
            classpath: true,
            native: false,
        }],
        metadata: Vec::new(),
    }
}

#[async_std::test]
async fn collects_dead_owned_unreferenced_generation_and_preserves_current_profile() {
    let f = fixture().await;
    let old = generation(&f).await;
    let current = generation(&f).await;
    storage::publish(&f.db, 1.into(), &current_profile(&current))
        .await
        .unwrap();
    assert_eq!(collect(f.paths.clone(), Duration::ZERO).await.unwrap(), 1);
    assert!(!f.paths.libraries().join(&old).exists());
    assert_eq!(
        fs::read(f.paths.libraries().join(&current).join("maven/library.jar")).unwrap(),
        b"runtime bytes"
    );
    assert_eq!(collect(f.paths.clone(), Duration::ZERO).await.unwrap(), 0);
}

#[async_std::test]
async fn roots_of_other_catalogued_database_and_legacy_placements_keep_generations() {
    let f = fixture().await;
    let first = generation(&f).await;
    let legacy = generation(&f).await;
    let second = Arc::new(
        Database::init(&format!(
            "sqlite://{}",
            f.directory.path().join("two.db").display()
        ))
        .await
        .unwrap(),
    );
    sqlx::query("INSERT INTO instances(id,name,dir,version,loader,manifest_url,meta_provider) VALUES(1,'other','other','1.21','vanilla','https://example.invalid','prism')").execute(&second.pool).await.unwrap();
    retention::attach(
        second.clone(),
        f.paths.libraries().clone(),
        Cancellation::new(),
    )
    .await
    .unwrap();
    storage::publish(&second, 1.into(), &current_profile(&first))
        .await
        .unwrap();
    sqlx::query("INSERT INTO instance_library_placements(instance_id,position,hash,name,path,native,url) VALUES(1,0,?,'legacy',?,0,'https://example.invalid')")
        .bind(format!("{:x}", sha1::Sha1::digest(b"runtime bytes"))).bind(f.paths.libraries().join(&legacy).join("maven/library.jar").to_str().unwrap()).execute(&f.db.pool).await.unwrap();
    assert_eq!(collect(f.paths.clone(), Duration::ZERO).await.unwrap(), 0);
    assert!(f.paths.libraries().join(first).is_dir() && f.paths.libraries().join(legacy).is_dir());
}

#[async_std::test]
async fn unknown_old_replaced_future_or_live_owners_and_fresh_outputs_are_preserved() {
    for mode in [
        "missing",
        "future-version",
        "wrong-database",
        "wrong-layout",
        "wrong-pid",
        "live-owner",
        "future-created",
        "future-output",
        "unknown-file",
    ] {
        let f = fixture().await;
        let relative = generation(&f).await;
        let root = f.paths.libraries().join(&relative);
        let mut owner = record(&root).unwrap();
        match mode {
            "missing" => fs::remove_file(root.join(RECORD)).unwrap(),
            "future-version" => {
                owner.version = 99;
                owner.write(&root).unwrap();
            }
            "wrong-database" => {
                owner.database = Some("ab".repeat(16));
                owner.write(&root).unwrap();
            }
            "wrong-layout" => {
                owner.generation = uuid::Uuid::new_v4();
                owner.write(&root).unwrap();
            }
            "wrong-pid" => fs::write(root.join(OWNER), std::process::id().to_string()).unwrap(),
            "live-owner" => {
                owner.pid = std::process::id();
                owner.write(&root).unwrap();
                fs::write(root.join(OWNER), owner.pid.to_string()).unwrap();
            }
            "future-created" => {
                owner.created += 86400;
                owner.write(&root).unwrap();
            }
            "future-output" => fs::File::open(root.join("maven/library.jar"))
                .unwrap()
                .set_times(
                    fs::FileTimes::new()
                        .set_modified(SystemTime::now() + Duration::from_secs(86400)),
                )
                .unwrap(),
            _ => fs::write(root.join("user-file"), b"unknown").unwrap(),
        }
        assert_eq!(
            collect(f.paths.clone(), Duration::ZERO).await.unwrap(),
            0,
            "{mode}"
        );
        assert!(root.join("maven/library.jar").is_file(), "{mode}");
    }
}

fn age_files(root: &Path, time: SystemTime) {
    for entry in fs::read_dir(root).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_dir() {
            age_files(&entry.path(), time);
        } else {
            fs::File::open(entry.path())
                .unwrap()
                .set_times(fs::FileTimes::new().set_modified(time))
                .unwrap();
        }
    }
}

#[async_std::test]
async fn minimum_age_applies_to_latest_output_as_well_as_generation_creation() {
    let f = fixture().await;
    let relative = generation(&f).await;
    let root = f.paths.libraries().join(relative);
    assert_eq!(
        collect(f.paths.clone(), Duration::from_secs(7 * 86400))
            .await
            .unwrap(),
        0
    );
    let mut owner = record(&root).unwrap();
    owner.created -= 9 * 86400;
    owner.write(&root).unwrap();
    age_files(&root, SystemTime::now() - Duration::from_secs(9 * 86400));
    fs::write(root.join("maven/library.jar"), b"fresh output").unwrap();
    assert_eq!(
        collect(f.paths.clone(), Duration::from_secs(7 * 86400))
            .await
            .unwrap(),
        0
    );
    age_files(&root, SystemTime::now() - Duration::from_secs(9 * 86400));
    assert_eq!(
        collect(f.paths.clone(), Duration::from_secs(7 * 86400))
            .await
            .unwrap(),
        1
    );
}

#[async_std::test]
async fn live_and_unknown_processor_pins_keep_entire_generation_not_only_work() {
    for unknown in [false, true] {
        let f = fixture().await;
        let relative = generation(&f).await;
        let root = f.paths.libraries().join(relative);
        let pin = process_pin::ProcessPin::create(&root).unwrap();
        pin.before_spawn().unwrap();
        if !unknown {
            pin.record_child(std::process::id());
        }
        assert_eq!(collect(f.paths.clone(), Duration::ZERO).await.unwrap(), 0);
        assert!(
            root.join("maven/library.jar").is_file() && root.join("work/installer.log").is_file()
        );
        pin.finish(true);
        assert_eq!(collect(f.paths.clone(), Duration::ZERO).await.unwrap(), 1);
    }
}

#[async_std::test]
async fn broad_game_pin_blocks_all_generation_deletion() {
    let f = fixture().await;
    let relative = generation(&f).await;
    let pin = process_pin::ProcessPin::create(f.paths.libraries()).unwrap();
    assert!(collect(f.paths.clone(), Duration::ZERO).await.is_err());
    assert!(f.paths.libraries().join(relative).is_dir());
    pin.finish(true);
    assert_eq!(collect(f.paths.clone(), Duration::ZERO).await.unwrap(), 1);
}

#[async_std::test]
async fn uncatalogued_writer_is_rejected_before_generation_outputs() {
    let f = fixture().await;
    fs::remove_file(f.paths.libraries().join(".sonata-roots.json")).unwrap();
    assert!(
        InstallWorkspace::create_owned(&f.paths, 1.into(), &Cancellation::new(), &f.db)
            .await
            .is_err()
    );
    assert!(!f.paths.libraries().join("instances").exists());
}

#[async_std::test]
async fn stale_launcher_owner_with_live_or_unknown_child_keeps_all_runtime_outputs() {
    for child in [Some(std::process::id()), None] {
        let f = fixture().await;
        let relative = generation(&f).await;
        let root = f.paths.libraries().join(relative);
        let owner = record(&root).unwrap();
        let pin = root
            .join(".sonata-processes")
            .join(uuid::Uuid::new_v4().to_string());
        fs::create_dir_all(&pin).unwrap();
        fs::write(pin.join(".sonata-owner"), owner.pid.to_string()).unwrap();
        fs::write(pin.join(".sonata-starting"), b"1").unwrap();
        if let Some(child) = child {
            fs::write(pin.join(".sonata-child"), child.to_string()).unwrap();
        }
        assert_eq!(collect(f.paths.clone(), Duration::ZERO).await.unwrap(), 0);
        assert!(
            root.join("maven/library.jar").is_file() && root.join("work/installer.log").is_file()
        );
    }
}

#[async_std::test]
async fn replay_rechecks_age_of_outputs_written_after_the_intent() {
    let f = fixture().await;
    let relative = generation(&f).await;
    let root = f.paths.libraries().join(relative);
    let mut owner = record(&root).unwrap();
    owner.created -= 9 * 86400;
    owner.write(&root).unwrap();
    age_files(&root, SystemTime::now() - Duration::from_secs(9 * 86400));
    child(&f, "crash_collector", "intent", 77).await;
    fs::write(root.join("maven/library.jar"), b"new output after crash").unwrap();
    assert_eq!(
        collect(f.paths.clone(), Duration::from_secs(7 * 86400))
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        fs::read(root.join("maven/library.jar")).unwrap(),
        b"new output after crash"
    );
    age_files(&root, SystemTime::now() - Duration::from_secs(9 * 86400));
    assert_eq!(
        collect(f.paths.clone(), Duration::from_secs(7 * 86400))
            .await
            .unwrap(),
        1
    );
}

#[cfg(unix)]
#[async_std::test]
async fn generation_tree_and_quarantine_symlinks_never_delete_outside_data() {
    for mode in ["nested", "queue-parent", "queue", "generation", "instance"] {
        let f = fixture().await;
        let relative = generation(&f).await;
        let original = f.paths.libraries().join(&relative);
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("user-bytes"), b"keep").unwrap();
        match mode {
            "nested" => {
                std::os::unix::fs::symlink(outside.path(), original.join("maven/linked")).unwrap()
            }
            "queue-parent" => std::os::unix::fs::symlink(
                outside.path(),
                f.paths.libraries().join(".sonata-runtime-gc"),
            )
            .unwrap(),
            "queue" => {
                fs::create_dir_all(f.paths.libraries().join(".sonata-runtime-gc")).unwrap();
                std::os::unix::fs::symlink(outside.path(), f.paths.libraries().join(QUEUE))
                    .unwrap();
            }
            _ => {
                let target = if mode == "instance" {
                    original.parent().unwrap().to_owned()
                } else {
                    original.clone()
                };
                fs::rename(&target, outside.path().join("saved")).unwrap();
                std::os::unix::fs::symlink(outside.path().join("saved"), target).unwrap();
            }
        }
        let result = collect(f.paths.clone(), Duration::ZERO).await;
        if matches!(mode, "queue" | "queue-parent") {
            assert!(result.is_err());
        } else {
            assert_eq!(result.unwrap(), 0, "{mode}");
        }
        assert_eq!(
            fs::read(outside.path().join("user-bytes")).unwrap(),
            b"keep"
        );
        assert!(original.join("maven/library.jar").is_file(), "{mode}");
    }
}

#[test]
fn crash_collector() {
    let Some(root) = std::env::var_os(ROOT).map(PathBuf::from) else {
        return;
    };
    if std::env::var(PHASE).ok().as_deref() == Some("writer") {
        return;
    }
    async_std::task::block_on(async {
        collect(paths(&root).await, Duration::ZERO).await.unwrap();
        panic!("missed generation GC crash checkpoint");
    });
}

#[async_std::test]
async fn process_exit_replays_intent_rename_output_metadata_and_completion_boundaries() {
    for phase in [
        "intent", "rename", "outputs", "metadata", "removed", "cleaned",
    ] {
        let f = fixture().await;
        let relative = generation(&f).await;
        child(&f, "crash_collector", phase, 77).await;
        let root = f.paths.libraries().join(&relative);
        assert_eq!(root.exists(), phase == "intent", "{phase}");
        collect(f.paths.clone(), Duration::ZERO).await.unwrap();
        assert!(!root.exists(), "{phase}");
        assert_eq!(
            fs::read_dir(f.paths.libraries().join(QUEUE))
                .unwrap()
                .count(),
            0,
            "{phase}"
        );
        assert_eq!(
            collect(f.paths.clone(), Duration::ZERO).await.unwrap(),
            0,
            "{phase}"
        );
    }
}

#[async_std::test]
async fn new_current_root_after_crash_cancels_intent_and_restores_quarantined_bytes() {
    for phase in ["intent", "rename"] {
        let f = fixture().await;
        let relative = generation(&f).await;
        child(&f, "crash_collector", phase, 77).await;
        storage::publish(&f.db, 1.into(), &current_profile(&relative))
            .await
            .unwrap();
        assert_eq!(collect(f.paths.clone(), Duration::ZERO).await.unwrap(), 0);
        assert_eq!(
            fs::read(
                f.paths
                    .libraries()
                    .join(&relative)
                    .join("maven/library.jar")
            )
            .unwrap(),
            b"runtime bytes"
        );
        assert_eq!(
            fs::read_dir(f.paths.libraries().join(QUEUE))
                .unwrap()
                .count(),
            0
        );
    }
}

#[async_std::test]
async fn corrupt_future_intent_and_replaced_generation_do_not_authorize_replay() {
    for mode in ["json", "version", "replacement"] {
        let f = fixture().await;
        let relative = generation(&f).await;
        child(&f, "crash_collector", "intent", 77).await;
        let root = f.paths.libraries().join(&relative);
        let owner = record(&root).unwrap();
        let intent = f
            .paths
            .libraries()
            .join(QUEUE)
            .join(format!("{}.json", owner.generation));
        match mode {
            "json" => fs::write(intent, b"broken").unwrap(),
            "version" => {
                let value = Intent { version: 99, owner };
                fs::write(intent, serde_json::to_vec(&value).unwrap()).unwrap();
            }
            _ => {
                let mut changed = owner;
                changed.created += 1;
                changed.write(&root).unwrap();
            }
        }
        assert!(
            collect(f.paths.clone(), Duration::ZERO).await.is_err(),
            "{mode}"
        );
        assert!(root.join("maven/library.jar").is_file(), "{mode}");
    }
}

type HookKey = (PathBuf, &'static str);
struct Hook {
    entered: futures::channel::oneshot::Sender<()>,
    resume: futures::channel::oneshot::Receiver<()>,
}
static HOOKS: std::sync::OnceLock<std::sync::Mutex<HashMap<HookKey, Hook>>> =
    std::sync::OnceLock::new();

pub(super) async fn checkpoint(root: &Path, phase: &'static str) {
    if std::env::var_os(ROOT).is_some_and(|base| Path::new(&base).join("libraries") == root)
        && std::env::var(PHASE).ok().as_deref() == Some(phase)
    {
        std::process::exit(77);
    }
    let hook = HOOKS
        .get_or_init(Default::default)
        .lock()
        .unwrap()
        .remove(&(root.to_owned(), phase));
    if let Some(hook) = hook {
        let _ = hook.entered.send(());
        let _ = hook.resume.await;
    }
}

#[async_std::test]
async fn dropping_collector_caller_keeps_snapshot_and_sql_locks_through_blocking_removal() {
    let f = fixture().await;
    let relative = generation(&f).await;
    let (entered, receiver) = futures::channel::oneshot::channel();
    let (resume, wait) = futures::channel::oneshot::channel();
    let key = (f.paths.libraries().clone(), "snapshot");
    HOOKS.get_or_init(Default::default).lock().unwrap().insert(
        key,
        Hook {
            entered,
            resume: wait,
        },
    );
    let mut future = Box::pin(collect(f.paths.clone(), Duration::ZERO));
    match futures::future::select(receiver, future.as_mut()).await {
        futures::future::Either::Left((result, _)) => result.unwrap(),
        futures::future::Either::Right((result, _)) => {
            panic!("collector completed early: {result:?}")
        }
    }
    drop(future);
    let store = crate::utils::object_store::ObjectStore::new(f.paths.libraries().clone());
    assert!(store.try_access().is_err());
    let probe = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(
            sqlx::sqlite::SqliteConnectOptions::new()
                .filename(f.directory.path().join("owner.db"))
                .busy_timeout(Duration::ZERO),
        )
        .await
        .unwrap();
    assert!(probe.begin_with("BEGIN IMMEDIATE").await.is_err());
    resume.send(()).unwrap();
    store.access(&Cancellation::new()).await.unwrap();
    assert!(!f.paths.libraries().join(relative).exists());
    probe.close().await;
}
