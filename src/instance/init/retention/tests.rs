//! Persistent library/SQLite fixtures. No network, JVM or deletion is required
//! to exercise the complete ownership/current-roots gate.
use super::*;
use crate::{
    data::config::Config,
    instance::init::{model::InstalledArtifact, storage, workspace::InstallWorkspace},
    java::policy::JavaPolicy,
    modloader::model::LoaderSpec,
};
use std::{fs, time::Duration};

struct Fixture {
    directory: tempfile::TempDir,
    paths: Arc<LauncherPaths>,
    db: Arc<Database>,
}

async fn database(path: &Path) -> Arc<Database> {
    let db = Arc::new(
        Database::init(&format!("sqlite://{}", path.display()))
            .await
            .unwrap(),
    );
    sqlx::query("INSERT INTO instances(id,name,dir,version,loader,manifest_url,meta_provider) VALUES(1,'runtime','runtime','1.21','vanilla','https://example.invalid','prism')").execute(&db.pool).await.unwrap();
    db
}

async fn fixture() -> Fixture {
    let directory = tempfile::tempdir().unwrap();
    let paths = Arc::new(
        LauncherPaths::resolve(&Config {
            root_override: Some(directory.path().to_owned()),
            ..Config::init()
        })
        .await
        .unwrap(),
    );
    let db = database(&directory.path().join("one.db")).await;
    attach(db.clone(), paths.libraries().clone(), Cancellation::new())
        .await
        .unwrap();
    Fixture {
        directory,
        paths,
        db,
    }
}

fn profile(id: i64) -> InstalledProfile {
    InstalledProfile {
        format_version: 1,
        minecraft: "1.21".into(),
        loader: LoaderSpec::vanilla(),
        manifest: serde_json::json!({}),
        java: JavaPolicy {
            majors: vec![21],
            allow_newer: false,
        },
        libraries_root: PathBuf::from("instances")
            .join(id.to_string())
            .join(uuid::Uuid::new_v4().to_string())
            .join("maven"),
        artifacts: vec![InstalledArtifact {
            path: "example/library.jar".into(),
            sha1: "ab".repeat(20),
            classpath: false,
            native: true,
        }],
        metadata: Vec::new(),
    }
}

async fn placement(db: &Database, path: &Path, hash: &str) {
    sqlx::query("INSERT INTO instance_library_placements(instance_id,position,hash,name,path,native,url) VALUES(1,0,?,'legacy',?,0,'https://example.invalid')")
        .bind(hash).bind(path.to_str().unwrap()).execute(&db.pool).await.unwrap();
}

#[async_std::test]
async fn all_catalogued_current_profiles_and_legacy_hashes_are_roots_even_if_files_missing() {
    let f = fixture().await;
    let second = database(&f.directory.path().join("two.db")).await;
    attach(
        second.clone(),
        f.paths.libraries().clone(),
        Cancellation::new(),
    )
    .await
    .unwrap();
    let first = profile(1);
    let mut other = profile(1);
    other.artifacts[0].sha1 = "cd".repeat(20);
    storage::publish(&f.db, 1.into(), &first).await.unwrap();
    storage::publish(&second, 1.into(), &other).await.unwrap();
    let legacy = profile(1).libraries_root.join("legacy.jar");
    placement(
        &second,
        &f.paths.libraries().join(&legacy),
        &"ef".repeat(20),
    )
    .await;
    let snapshot = snapshot(&f.paths).await.unwrap();
    assert_eq!(
        snapshot.generations,
        [
            first.libraries_root.parent().unwrap().to_owned(),
            other.libraries_root.parent().unwrap().to_owned(),
            legacy.parent().unwrap().parent().unwrap().to_owned()
        ]
        .into_iter()
        .collect()
    );
    assert_eq!(
        snapshot.sha1,
        ["ab".repeat(20), "cd".repeat(20), "ef".repeat(20)]
            .into_iter()
            .collect()
    );
    snapshot.databases.close().await;
}

#[async_std::test]
async fn linked_old_sql_snapshot_is_retained_without_using_aliases_as_roots() {
    let f = fixture().await;
    let path = profile(1).libraries_root.join("old.jar");
    sqlx::query("INSERT INTO libraries(hash,name,path,native,url) VALUES(?,'old',?,0,'https://example.invalid')")
        .bind("ac".repeat(20)).bind(f.paths.libraries().join(&path).to_str().unwrap()).execute(&f.db.pool).await.unwrap();
    sqlx::query("INSERT INTO instances_libraries(instance_id,library_id,position) SELECT 1,id,0 FROM libraries").execute(&f.db.pool).await.unwrap();
    fs::create_dir_all(f.paths.libraries().join("blobs/aliases/sha1")).unwrap();
    fs::write(
        f.paths
            .libraries()
            .join("blobs/aliases/sha1")
            .join("df".repeat(20)),
        "12".repeat(32),
    )
    .unwrap();
    let snapshot = snapshot(&f.paths).await.unwrap();
    assert_eq!(snapshot.sha1, ["ac".repeat(20)].into_iter().collect());
    assert!(snapshot
        .generations
        .contains(path.parent().unwrap().parent().unwrap()));
    snapshot.databases.close().await;
}

#[async_std::test]
async fn unknown_profiles_paths_versions_hashes_and_fields_fail_closed() {
    for mode in [
        "json",
        "version",
        "extra-root",
        "extra-artifact",
        "absolute-root",
        "escape-root",
        "unknown-layout",
        "non-maven",
        "foreign-instance",
        "short-generation",
        "bad-uuid",
        "escape-artifact",
        "absolute-artifact",
        "bad-hash",
    ] {
        let f = fixture().await;
        let p = profile(1);
        let mut value = serde_json::to_value(&p).unwrap();
        match mode {
            "version" => value["format_version"] = 99.into(),
            "extra-root" => value["additional_root"] = "unknown".into(),
            "extra-artifact" => value["artifacts"][0]["additional_root"] = "unknown".into(),
            "absolute-root" => value["libraries_root"] = "/external/maven".into(),
            "escape-root" => value["libraries_root"] = "../outside".into(),
            "unknown-layout" => value["libraries_root"] = "old-maven".into(),
            "non-maven" => {
                value["libraries_root"] = p
                    .libraries_root
                    .parent()
                    .unwrap()
                    .join("other")
                    .to_str()
                    .unwrap()
                    .into()
            }
            "foreign-instance" => {
                value["libraries_root"] = profile(2).libraries_root.to_str().unwrap().into()
            }
            "short-generation" => value["libraries_root"] = "instances/1/maven".into(),
            "bad-uuid" => value["libraries_root"] = "instances/1/not-a-uuid/maven".into(),
            "escape-artifact" => value["artifacts"][0]["path"] = "../outside.jar".into(),
            "absolute-artifact" => value["artifacts"][0]["path"] = "/outside.jar".into(),
            "bad-hash" => value["artifacts"][0]["sha1"] = "invalid".into(),
            _ => {}
        }
        let body = if mode == "json" {
            "broken".to_owned()
        } else {
            serde_json::to_string(&value).unwrap()
        };
        sqlx::query("INSERT INTO instance_installations(instance_id,profile_json) VALUES(1,?)")
            .bind(body)
            .execute(&f.db.pool)
            .await
            .unwrap();
        assert!(snapshot(&f.paths).await.is_err(), "{mode}");
    }
}

#[async_std::test]
async fn unknown_relocated_or_malformed_legacy_placements_block_the_gate() {
    for (path, hash) in [
        ("/unknown/library.jar", "ab".repeat(20)),
        ("../outside.jar", "ab".repeat(20)),
        ("instances/1/unknown/maven/lib.jar", "ab".repeat(20)),
        ("legacy/lib.jar", "zz".repeat(20)),
    ] {
        let f = fixture().await;
        placement(&f.db, Path::new(path), &hash).await;
        assert!(snapshot(&f.paths).await.is_err(), "{path} {hash}");
    }
}

#[async_std::test]
async fn unavailable_replaced_or_reattached_historical_database_never_becomes_a_fresh_owner() {
    for mode in ["missing", "replaced", "reattached"] {
        let f = fixture().await;
        let second_path = f.directory.path().join("two.db");
        let second = database(&second_path).await;
        attach(
            second.clone(),
            f.paths.libraries().clone(),
            Cancellation::new(),
        )
        .await
        .unwrap();
        second.pool.close().await;
        fs::rename(&second_path, f.directory.path().join("previous.db")).unwrap();
        if mode != "missing" {
            let replacement = database(&second_path).await;
            if mode == "reattached" {
                attach(
                    replacement,
                    f.paths.libraries().clone(),
                    Cancellation::new(),
                )
                .await
                .unwrap();
            }
        }
        assert!(snapshot(&f.paths).await.is_err(), "{mode}");
        assert_eq!(second_path.exists(), mode != "missing");
    }
}

#[async_std::test]
async fn ephemeral_unknown_and_uncatalogued_historical_bytes_permanently_disable_collection() {
    for mode in ["memory", "unknown-db", "historical"] {
        let f = fixture().await;
        match mode {
            "memory" => attach(
                Arc::new(Database::init("sqlite::memory:").await.unwrap()),
                f.paths.libraries().clone(),
                Cancellation::new(),
            )
            .await
            .unwrap(),
            "unknown-db" => {
                let pool = sqlx::sqlite::SqlitePoolOptions::new()
                    .connect_with(
                        sqlx::sqlite::SqliteConnectOptions::new()
                            .filename(f.directory.path().join("unknown.db"))
                            .create_if_missing(true),
                    )
                    .await
                    .unwrap();
                attach(
                    Arc::new(Database { pool }),
                    f.paths.libraries().clone(),
                    Cancellation::new(),
                )
                .await
                .unwrap();
            }
            _ => {
                fs::remove_file(f.paths.libraries().join(".sonata-roots.json")).unwrap();
                fs::write(f.paths.libraries().join("historical"), b"unknown owner").unwrap();
            }
        }
        attach(
            f.db.clone(),
            f.paths.libraries().clone(),
            Cancellation::new(),
        )
        .await
        .unwrap();
        assert!(snapshot(&f.paths).await.is_err(), "{mode}");
    }
}

#[async_std::test]
async fn broad_game_pin_blocks_snapshot_until_confirmed_exit() {
    let f = fixture().await;
    let pin = process_pin::ProcessPin::create(f.paths.libraries()).unwrap();
    pin.before_spawn().unwrap();
    pin.record_child(std::process::id());
    assert!(snapshot(&f.paths).await.is_err());
    pin.finish(false);
    assert!(snapshot(&f.paths).await.is_err());
    pin.finish(true);
    snapshot(&f.paths).await.unwrap().databases.close().await;
}

#[async_std::test]
async fn catalogue_precedes_workspace_outputs_and_known_writers_do_not_need_exclusive_readmission()
{
    let f = fixture().await;
    let workspace = InstallWorkspace::create(&f.paths, 1.into()).await.unwrap();
    assert_eq!(
        RootCatalog::covers(
            f.paths.libraries(),
            f.db.retention_owner().await.unwrap().as_ref()
        )
        .unwrap(),
        true
    );
    async_std::future::timeout(
        Duration::from_secs(2),
        attach(
            f.db.clone(),
            f.paths.libraries().clone(),
            Cancellation::new(),
        ),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(snapshot(&f.paths).await.is_err());
    workspace.cleanup_failed().await.unwrap();
    drop(workspace);
    snapshot(&f.paths).await.unwrap().databases.close().await;
}

#[async_std::test]
async fn cancelled_admission_creates_neither_catalogue_nor_generation() {
    let f = fixture().await;
    fs::remove_file(f.paths.libraries().join(".sonata-roots.json")).unwrap();
    let store = ObjectStore::new(f.paths.libraries().clone());
    let guard = store.try_maintenance().unwrap();
    let cancel = Cancellation::new();
    cancel.cancel();
    assert!(attach(f.db.clone(), f.paths.libraries().clone(), cancel)
        .await
        .is_err());
    assert!(!guard.root().join(".sonata-roots.json").exists());
    assert!(!guard.root().join("instances").exists());
}

#[async_std::test]
async fn corrupt_or_future_catalogue_refuses_new_writer_and_does_not_change_metadata() {
    for body in ["broken", "{\"version\":99,\"complete\":true,\"owners\":[]}"] {
        let f = fixture().await;
        let path = f.paths.libraries().join(".sonata-roots.json");
        fs::write(&path, body).unwrap();
        assert!(attach(
            f.db.clone(),
            f.paths.libraries().clone(),
            Cancellation::new()
        )
        .await
        .is_err());
        assert!(snapshot(&f.paths).await.is_err());
        assert_eq!(fs::read_to_string(path).unwrap(), body);
    }
}

#[async_std::test]
async fn snapshot_owns_sql_locks_and_maintenance_until_closed() {
    let f = fixture().await;
    let snapshot = snapshot(&f.paths).await.unwrap();
    let store = ObjectStore::new(f.paths.libraries().clone());
    assert!(store.try_access().is_err());
    let probe = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(
            sqlx::sqlite::SqliteConnectOptions::new()
                .filename(f.directory.path().join("one.db"))
                .busy_timeout(Duration::ZERO),
        )
        .await
        .unwrap();
    assert!(probe.begin_with("BEGIN IMMEDIATE").await.is_err());
    snapshot.databases.close().await;
    assert!(store.try_access().is_err()); // independent snapshot capability
    drop(snapshot.maintenance);
    assert!(store.try_access().is_ok());
    probe
        .begin_with("BEGIN IMMEDIATE")
        .await
        .unwrap()
        .rollback()
        .await
        .unwrap();
    probe.close().await;
}

#[cfg(unix)]
#[async_std::test]
async fn canonical_library_alias_shares_catalogue_but_escaped_generation_links_block_roots() {
    let f = fixture().await;
    let alias = f.directory.path().join("library-alias");
    std::os::unix::fs::symlink(f.paths.libraries(), &alias).unwrap();
    attach(f.db.clone(), alias.clone(), Cancellation::new())
        .await
        .unwrap();
    let guard = ObjectStore::new(alias).try_maintenance().unwrap();
    assert_eq!(RootCatalog::owners(&guard).unwrap().len(), 1);
    drop(guard);
    let p = profile(1);
    storage::publish(&f.db, 1.into(), &p).await.unwrap();
    fs::create_dir_all(f.paths.libraries().join("instances/1")).unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink(
        outside.path(),
        f.paths.libraries().join(p.libraries_root.parent().unwrap()),
    )
    .unwrap();
    assert!(snapshot(&f.paths).await.is_err());
}

type HookKey = (PathBuf, &'static str);
struct Hook {
    entered: futures::channel::oneshot::Sender<()>,
    resume: futures::channel::oneshot::Receiver<()>,
}
static HOOKS: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<HookKey, Hook>>> =
    std::sync::OnceLock::new();

pub(super) async fn checkpoint(root: &Path, phase: &'static str) {
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
async fn dropped_attach_caller_keeps_owned_exclusive_gate_until_catalogue_is_durable() {
    let f = fixture().await;
    fs::remove_file(f.paths.libraries().join(".sonata-roots.json")).unwrap();
    let (entered, receiver) = futures::channel::oneshot::channel();
    let (resume, wait) = futures::channel::oneshot::channel();
    HOOKS.get_or_init(Default::default).lock().unwrap().insert(
        (f.paths.libraries().clone(), "attach"),
        Hook {
            entered,
            resume: wait,
        },
    );
    let mut future = Box::pin(attach(
        f.db.clone(),
        f.paths.libraries().clone(),
        Cancellation::new(),
    ));
    match futures::future::select(receiver, future.as_mut()).await {
        futures::future::Either::Left((result, _)) => result.unwrap(),
        futures::future::Either::Right((result, _)) => panic!("attach completed early: {result:?}"),
    }
    drop(future);
    let store = ObjectStore::new(f.paths.libraries().clone());
    assert!(store.try_access().is_err());
    resume.send(()).unwrap();
    let access = store.access(&Cancellation::new()).await.unwrap();
    assert!(RootCatalog::covers(
        access.root(),
        f.db.retention_owner().await.unwrap().as_ref()
    )
    .unwrap());
}
