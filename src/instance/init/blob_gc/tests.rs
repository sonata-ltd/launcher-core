use super::*;
use crate::{
    data::{config::Config, db::Database},
    instance::init::{
        model::{InstalledArtifact, InstalledProfile},
        storage,
    },
    java::policy::JavaPolicy,
    modloader::model::LoaderSpec,
    utils::{
        cancellation::Cancellation,
        object_store::{
            collection::{DeleteIntent, INTENTS},
            StoredObject,
        },
    },
};
use std::path::{Path, PathBuf};

struct Fixture {
    directory: tempfile::TempDir,
    paths: Arc<LauncherPaths>,
    db: Arc<Database>,
    store: Arc<ObjectStore>,
}

async fn database(path: &Path) -> Arc<Database> {
    let db = Arc::new(
        Database::init(&format!("sqlite://{}", path.display()))
            .await
            .unwrap(),
    );
    sqlx::query("INSERT INTO instances(id,name,dir,version,loader,manifest_url,meta_provider) VALUES(1,'blob','blob','1.21','vanilla','https://example.invalid','prism')").execute(&db.pool).await.unwrap();
    db
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
    let db = database(&directory.path().join("owner.db")).await;
    retention::attach(db.clone(), paths.libraries().clone(), Cancellation::new())
        .await
        .unwrap();
    let store = Arc::new(ObjectStore::new(paths.libraries().join("blobs")));
    Fixture {
        directory,
        paths,
        db,
        store,
    }
}

impl Fixture {
    fn import(&self, body: &[u8]) -> StoredObject {
        let source = self.directory.path().join("source");
        fs::write(&source, body).unwrap();
        self.store
            .import_file(&source, None, &Cancellation::new())
            .unwrap()
    }

    async fn generation(&self) -> PathBuf {
        let root = self.directory.path().to_owned();
        let output = async_std::task::spawn_blocking(move || {
            std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "instance::init::generation_gc::tests::generation_writer",
                    "--nocapture",
                ])
                .env("SONATA_TEST_GENERATION_ROOT", root)
                .env("SONATA_TEST_GENERATION_PHASE", "writer")
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
        PathBuf::from(fs::read_to_string(self.directory.path().join("last-generation")).unwrap())
    }
}

fn profile(hash: &str) -> InstalledProfile {
    InstalledProfile {
        format_version: 1,
        minecraft: "1.21".into(),
        loader: LoaderSpec::vanilla(),
        manifest: serde_json::json!({}),
        java: JavaPolicy {
            majors: vec![21],
            allow_newer: false,
        },
        libraries_root: PathBuf::from("instances/1")
            .join(uuid::Uuid::new_v4().to_string())
            .join("maven"),
        artifacts: vec![InstalledArtifact {
            path: "missing.jar".into(),
            sha1: hash.to_owned(),
            classpath: false,
            native: false,
        }],
        metadata: Vec::new(),
    }
}

fn intent(f: &Fixture, object: &StoredObject) -> PathBuf {
    f.store
        .root()
        .join(INTENTS)
        .join(format!("{}.json", object.digests.sha256))
}

#[async_std::test]
async fn collects_only_old_unrooted_blobs_and_keeps_profile_checksums_with_missing_files() {
    let f = fixture().await;
    let rooted = f.import(b"rooted");
    let garbage = f.import(b"garbage");
    storage::publish(&f.db, 1.into(), &profile(&rooted.digests.sha1))
        .await
        .unwrap();
    assert_eq!(
        collect(f.paths.clone(), Duration::from_secs(7 * 86400))
            .await
            .unwrap(),
        0
    );
    assert_eq!(collect(f.paths.clone(), Duration::ZERO).await.unwrap(), 1);
    assert!(rooted.path.is_file() && !garbage.path.exists());
    assert_eq!(collect(f.paths.clone(), Duration::ZERO).await.unwrap(), 0);
}

#[async_std::test]
async fn all_catalogued_database_profiles_and_legacy_placements_keep_blob_bytes() {
    let f = fixture().await;
    let second = database(&f.directory.path().join("two.db")).await;
    retention::attach(
        second.clone(),
        f.paths.libraries().clone(),
        Cancellation::new(),
    )
    .await
    .unwrap();
    let rooted = f.import(b"other DB");
    let legacy = f.import(b"legacy");
    let garbage = f.import(b"unused");
    storage::publish(&second, 1.into(), &profile(&rooted.digests.sha1))
        .await
        .unwrap();
    sqlx::query("INSERT INTO instance_library_placements(instance_id,position,hash,name,path,native,url) VALUES(1,0,?,'legacy',?,0,'https://example.invalid')")
        .bind(&legacy.digests.sha1).bind(profile(&legacy.digests.sha1).libraries_root.join("legacy.jar").to_str().unwrap()).execute(&f.db.pool).await.unwrap();
    assert_eq!(collect(f.paths.clone(), Duration::ZERO).await.unwrap(), 1);
    assert!(rooted.path.is_file() && legacy.path.is_file() && !garbage.path.exists());
}

#[async_std::test]
async fn surviving_unreferenced_generations_keep_blobs_for_maven_and_work_bytes() {
    let f = fixture().await;
    let maven = f.import(b"runtime bytes");
    let work = f.import(b"diagnostics");
    let garbage = f.import(b"unused");
    let generation = f.generation().await;
    assert_eq!(collect(f.paths.clone(), Duration::ZERO).await.unwrap(), 1);
    assert!(maven.path.is_file() && work.path.is_file() && !garbage.path.exists());
    assert!(f.paths.libraries().join(&generation).is_dir());
    generation_gc::collect(f.paths.clone(), Duration::ZERO)
        .await
        .unwrap();
    assert_eq!(collect(f.paths.clone(), Duration::ZERO).await.unwrap(), 2);
}

#[async_std::test]
async fn live_unknown_or_future_generation_preparations_block_all_blobs_including_unmaterialized_inputs(
) {
    for mode in [
        "missing-owner",
        "unknown-db",
        "live-owner",
        "future-owner",
        "future-version",
        "unknown-field",
        "unknown-child",
        "live-child",
        "legacy-directory",
    ] {
        let f = fixture().await;
        let garbage = f.import(b"unmaterialized next input");
        let generation = f.generation().await;
        let root = f.paths.libraries().join(generation);
        let record = root.join(".sonata-generation.json");
        let mut value: serde_json::Value =
            serde_json::from_slice(&fs::read(&record).unwrap()).unwrap();
        match mode {
            "missing-owner" => fs::remove_file(record).unwrap(),
            "unknown-db" => {
                value["database"] = "ab".repeat(16).into();
                fs::write(record, serde_json::to_vec(&value).unwrap()).unwrap();
            }
            "live-owner" => {
                value["pid"] = std::process::id().into();
                fs::write(record, serde_json::to_vec(&value).unwrap()).unwrap();
                fs::write(root.join(".sonata-owner"), std::process::id().to_string()).unwrap();
            }
            "future-owner" => {
                value["created"] = (value["created"].as_u64().unwrap() + 86400).into();
                fs::write(record, serde_json::to_vec(&value).unwrap()).unwrap();
            }
            "future-version" => {
                value["version"] = 99.into();
                fs::write(record, serde_json::to_vec(&value).unwrap()).unwrap();
            }
            "unknown-field" => {
                value["additional_inputs"] = "unknown".into();
                fs::write(record, serde_json::to_vec(&value).unwrap()).unwrap();
            }
            "unknown-child" | "live-child" => {
                let pin = root
                    .join(".sonata-processes")
                    .join(uuid::Uuid::new_v4().to_string());
                fs::create_dir_all(&pin).unwrap();
                fs::write(
                    pin.join(".sonata-owner"),
                    value["pid"].as_u64().unwrap().to_string(),
                )
                .unwrap();
                fs::write(pin.join(".sonata-starting"), b"1").unwrap();
                if mode == "live-child" {
                    fs::write(pin.join(".sonata-child"), std::process::id().to_string()).unwrap();
                }
            }
            _ => fs::create_dir(f.paths.libraries().join("legacy-maven")).unwrap(),
        }
        assert!(
            collect(f.paths.clone(), Duration::ZERO).await.is_err(),
            "{mode}"
        );
        assert!(garbage.path.is_file(), "{mode}");
    }
}

#[async_std::test]
async fn corrupt_alias_and_url_indexes_cannot_replace_semantic_profile_roots() {
    let f = fixture().await;
    let rooted = f.import(b"profile root");
    let garbage = f.import(b"cache-only hint");
    storage::publish(&f.db, 1.into(), &profile(&rooted.digests.sha1))
        .await
        .unwrap();
    fs::write(
        f.store
            .root()
            .join("aliases/sha1")
            .join(&rooted.digests.sha1),
        &garbage.digests.sha256,
    )
    .unwrap();
    fs::create_dir_all(f.store.root().join("urls")).unwrap();
    fs::write(f.store.root().join("urls/index"), &garbage.digests.sha1).unwrap();
    let legacy = f.store.root().join(&garbage.digests.sha1);
    fs::write(&legacy, b"old SHA-1 layout").unwrap();
    assert_eq!(collect(f.paths.clone(), Duration::ZERO).await.unwrap(), 1);
    assert!(rooted.path.is_file() && !garbage.path.exists() && legacy.is_file());
    assert!(f.store.root().join("urls/index").is_file()); // hint may now be stale; consumers verify bytes
}

#[async_std::test]
async fn unavailable_owner_and_broad_game_pin_defer_all_blob_deletion() {
    for mode in ["owner", "game"] {
        let f = fixture().await;
        let object = f.import(b"keep");
        if mode == "owner" {
            f.db.pool.close().await;
            fs::rename(
                f.directory.path().join("owner.db"),
                f.directory.path().join("old.db"),
            )
            .unwrap();
        } else {
            crate::utils::process_pin::ProcessPin::create(f.paths.libraries()).unwrap();
        }
        assert!(
            collect(f.paths.clone(), Duration::ZERO).await.is_err(),
            "{mode}"
        );
        assert!(object.path.is_file());
    }
}

#[async_std::test]
async fn admitted_blob_importer_excludes_collection_even_without_a_library_guard() {
    let f = fixture().await;
    let object = f.import(b"reader");
    let reader = f.store.try_access().unwrap();
    assert!(collect(f.paths.clone(), Duration::ZERO).await.is_err());
    assert!(object.path.is_file());
    drop(reader);
    assert_eq!(collect(f.paths.clone(), Duration::ZERO).await.unwrap(), 1);
}

#[async_std::test]
async fn pending_corrupt_or_unowned_generation_quarantine_blocks_every_blob() {
    for mode in ["intent", "quarantine", "corrupt", "unowned", "unknown"] {
        let f = fixture().await;
        let object = f.import(b"pending generation input");
        let queue = f.paths.libraries().join(".sonata-runtime-gc/generations");
        if matches!(mode, "intent" | "quarantine") {
            f.generation().await;
            let root = f.directory.path().to_owned();
            let phase = if mode == "intent" { "intent" } else { "rename" };
            let output = async_std::task::spawn_blocking(move || {
                std::process::Command::new(std::env::current_exe().unwrap())
                    .args([
                        "--exact",
                        "instance::init::generation_gc::tests::crash_collector",
                        "--nocapture",
                    ])
                    .env("SONATA_TEST_GENERATION_ROOT", root)
                    .env("SONATA_TEST_GENERATION_PHASE", phase)
                    .output()
                    .unwrap()
            })
            .await;
            assert_eq!(
                output.status.code(),
                Some(77),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        } else {
            fs::create_dir_all(&queue).unwrap();
            match mode {
                "corrupt" => fs::write(
                    queue.join(format!("{}.json", uuid::Uuid::new_v4())),
                    b"broken",
                )
                .unwrap(),
                "unowned" => fs::create_dir(queue.join(uuid::Uuid::new_v4().to_string())).unwrap(),
                _ => {
                    fs::write(queue.parent().unwrap().join("unknown"), b"future metadata").unwrap()
                }
            }
        }
        assert!(
            collect(f.paths.clone(), Duration::ZERO).await.is_err(),
            "{mode}"
        );
        assert!(
            object.path.is_file() && !intent(&f, &object).exists(),
            "{mode}"
        );
    }
}

#[async_std::test]
async fn unknown_instance_and_generation_layouts_block_even_when_directories_are_empty() {
    for relative in [
        "instances/01",
        "instances/unknown",
        "instances/1/not-a-uuid",
        "instances/1/metadata",
    ] {
        let f = fixture().await;
        let object = f.import(b"unknown ownership");
        fs::create_dir_all(f.paths.libraries().join(relative)).unwrap();
        assert!(
            collect(f.paths.clone(), Duration::ZERO).await.is_err(),
            "{relative}"
        );
        assert!(object.path.is_file() && !intent(&f, &object).exists());
    }
}

#[async_std::test]
async fn future_blob_and_generation_output_timestamps_never_authorize_collection() {
    let f = fixture().await;
    let object = f.import(b"future blob");
    fs::File::open(&object.path)
        .unwrap()
        .set_times(
            fs::FileTimes::new()
                .set_modified(std::time::SystemTime::now() + Duration::from_secs(86400)),
        )
        .unwrap();
    assert_eq!(collect(f.paths.clone(), Duration::ZERO).await.unwrap(), 0);
    assert!(object.path.is_file());
    let unmaterialized = f.import(b"future preparation input");
    let generation = f.generation().await;
    fs::File::open(
        f.paths
            .libraries()
            .join(generation)
            .join("maven/library.jar"),
    )
    .unwrap()
    .set_times(
        fs::FileTimes::new()
            .set_modified(std::time::SystemTime::now() + Duration::from_secs(86400)),
    )
    .unwrap();
    assert!(collect(f.paths.clone(), Duration::ZERO).await.is_err());
    assert!(object.path.is_file() && unmaterialized.path.is_file());
}

fn age_files(root: &Path) {
    let time = std::time::SystemTime::now() - Duration::from_secs(9 * 86400);
    for entry in fs::read_dir(root).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_dir() {
            age_files(&entry.path());
        } else {
            fs::File::open(entry.path())
                .unwrap()
                .set_times(fs::FileTimes::new().set_modified(time))
                .unwrap();
        }
    }
}

#[async_std::test]
async fn startup_collects_generations_before_blobs_and_preserves_current_profile_bytes() {
    let f = fixture().await;
    let rooted = f.import(b"current profile");
    storage::publish(&f.db, 1.into(), &profile(&rooted.digests.sha1))
        .await
        .unwrap();
    let old_generation_blob = f.import(b"runtime bytes");
    let garbage = f.import(b"old unused");
    let fresh = f.import(b"recent unused");
    let relative = f.generation().await;
    let root = f.paths.libraries().join(&relative);
    let record = root.join(".sonata-generation.json");
    let mut value: serde_json::Value = serde_json::from_slice(&fs::read(&record).unwrap()).unwrap();
    value["created"] = (value["created"].as_u64().unwrap() - 9 * 86400).into();
    fs::write(record, serde_json::to_vec(&value).unwrap()).unwrap();
    age_files(&root);
    age_files(f.store.root());
    fs::write(&fresh.path, b"recent unused").unwrap();
    retention::initialize(f.db.clone(), f.paths.clone())
        .await
        .unwrap();
    assert!(!root.exists() && !old_generation_blob.path.exists() && !garbage.path.exists());
    assert!(rooted.path.is_file() && fresh.path.is_file());
    retention::initialize(f.db.clone(), f.paths.clone())
        .await
        .unwrap();
    assert!(rooted.path.is_file() && fresh.path.is_file());
}

#[cfg(unix)]
#[async_std::test]
async fn canonical_library_alias_uses_the_same_catalogue_and_blob_admission() {
    let f = fixture().await;
    let object = f.import(b"canonical blob root");
    let alias = f.directory.path().join("library-alias");
    std::os::unix::fs::symlink(f.paths.libraries(), &alias).unwrap();
    let aliased_paths = Arc::new(
        LauncherPaths::resolve(&Config {
            root_override: Some(f.directory.path().to_owned()),
            dirs: crate::data::config::DirOverrides {
                libraries: Some(alias),
                ..Default::default()
            },
            ..Config::init()
        })
        .await
        .unwrap(),
    );
    retention::attach(
        f.db.clone(),
        aliased_paths.libraries().clone(),
        Cancellation::new(),
    )
    .await
    .unwrap();
    let reader = f.store.try_access().unwrap();
    assert!(collect(aliased_paths.clone(), Duration::ZERO)
        .await
        .is_err());
    assert!(object.path.is_file());
    drop(reader);
    assert_eq!(collect(aliased_paths, Duration::ZERO).await.unwrap(), 1);
    assert!(!object.path.exists());
}

#[cfg(unix)]
#[async_std::test]
async fn linked_blob_roots_and_generation_files_preserve_outside_bytes() {
    for mode in ["blobs", "generation"] {
        let f = fixture().await;
        let object = f.import(b"preserve");
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("user-file"), b"keep").unwrap();
        if mode == "blobs" {
            fs::rename(f.store.root(), outside.path().join("saved")).unwrap();
            std::os::unix::fs::symlink(outside.path().join("saved"), f.store.root()).unwrap();
        } else {
            let generation = f.generation().await;
            std::os::unix::fs::symlink(
                outside.path(),
                f.paths.libraries().join(generation).join("maven/linked"),
            )
            .unwrap();
        }
        assert!(collect(f.paths.clone(), Duration::ZERO).await.is_err());
        assert!(object.path.is_file());
        assert_eq!(fs::read(outside.path().join("user-file")).unwrap(), b"keep");
    }
}

const ROOT: &str = "SONATA_TEST_BLOB_GC_ROOT";
const PHASE: &str = "SONATA_TEST_BLOB_GC_PHASE";

#[test]
fn crash_collector() {
    let Some(root) = std::env::var_os(ROOT).map(PathBuf::from) else {
        return;
    };
    async_std::task::block_on(async {
        collect(paths(&root).await, Duration::ZERO).await.unwrap();
        panic!("missed runtime blob crash checkpoint");
    });
}

async fn crash(f: &Fixture, phase: &str) {
    let root = f.directory.path().to_owned();
    let phase = phase.to_owned();
    let output = async_std::task::spawn_blocking(move || {
        std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "instance::init::blob_gc::tests::crash_collector",
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
        Some(78),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[async_std::test]
async fn process_exit_replays_blob_intent_unlink_alias_and_cleanup() {
    for phase in ["intent", "unlink", "alias", "cleaned"] {
        let f = fixture().await;
        let object = f.import(b"crash bytes");
        crash(&f, phase).await;
        assert_eq!(object.path.exists(), phase == "intent", "{phase}");
        collect(f.paths.clone(), Duration::ZERO).await.unwrap();
        assert!(!object.path.exists() && !intent(&f, &object).exists());
        assert!(f
            .store
            .find_sha1(&object.digests.sha1, &Cancellation::new())
            .unwrap()
            .is_none());
        assert_eq!(collect(f.paths.clone(), Duration::ZERO).await.unwrap(), 0);
    }
}

#[async_std::test]
async fn replay_rechecks_new_profile_roots_and_age_of_reimported_bytes() {
    let f = fixture().await;
    let object = f.import(b"new root");
    crash(&f, "intent").await;
    storage::publish(&f.db, 1.into(), &profile(&object.digests.sha1))
        .await
        .unwrap();
    assert_eq!(collect(f.paths.clone(), Duration::ZERO).await.unwrap(), 0);
    assert!(object.path.is_file() && !intent(&f, &object).exists());
    let garbage = f.import(b"reimport");
    crash(&f, "unlink").await;
    let fresh = f.import(b"reimport");
    assert_eq!(fresh.path, garbage.path);
    assert_eq!(
        collect(f.paths.clone(), Duration::from_secs(7 * 86400))
            .await
            .unwrap(),
        0
    );
    assert!(fresh.path.is_file() && intent(&f, &fresh).is_file());
    assert_eq!(collect(f.paths.clone(), Duration::ZERO).await.unwrap(), 1);
    assert!(object.path.is_file());
}

#[async_std::test]
async fn corrupt_or_future_blob_intents_preserve_data_and_metadata() {
    for version in [1, 99] {
        let f = fixture().await;
        let object = f.import(b"unknown intent");
        fs::create_dir_all(f.store.root().join(INTENTS)).unwrap();
        let body = if version == 1 {
            b"broken".to_vec()
        } else {
            serde_json::to_vec(&DeleteIntent {
                version,
                sha256: object.digests.sha256.clone(),
                sha1: object.digests.sha1.clone(),
                size: object.digests.size_bytes,
            })
            .unwrap()
        };
        fs::write(intent(&f, &object), &body).unwrap();
        assert!(collect(f.paths.clone(), Duration::ZERO).await.is_err());
        assert!(object.path.is_file());
        assert_eq!(fs::read(intent(&f, &object)).unwrap(), body);
    }
}

type HookKey = (PathBuf, &'static str);
struct Hook {
    entered: futures::channel::oneshot::Sender<()>,
    resume: futures::channel::oneshot::Receiver<()>,
}
static HOOKS: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<HookKey, Hook>>> =
    std::sync::OnceLock::new();

pub(super) async fn checkpoint(root: &Path, phase: &'static str) {
    if std::env::var_os(ROOT).is_some_and(|base| Path::new(&base).join("libraries/blobs") == root)
        && std::env::var(PHASE).ok().as_deref() == Some(phase)
    {
        std::process::exit(78);
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
async fn dropping_collector_caller_keeps_both_admissions_and_sql_snapshot_until_unlink_finishes() {
    for phase in ["snapshot", "intent"] {
        let f = fixture().await;
        let object = f.import(b"owned collector");
        let (entered, receiver) = futures::channel::oneshot::channel();
        let (resume, wait) = futures::channel::oneshot::channel();
        HOOKS.get_or_init(Default::default).lock().unwrap().insert(
            (f.store.root().to_owned(), phase),
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
        assert!(f.store.try_access().is_err());
        assert!(ObjectStore::new(f.paths.libraries().clone())
            .try_access()
            .is_err());
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
        ObjectStore::new(f.paths.libraries().clone())
            .access(&Cancellation::new())
            .await
            .unwrap();
        assert!(!object.path.exists());
        probe.close().await;
    }
}
