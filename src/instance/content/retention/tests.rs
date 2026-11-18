//! Persistent SQLite/CAS fixtures; no HTTP, JVM or power-cut simulation.
use super::*;
use crate::instance::{
    content::{
        install::checkpoints::Pause,
        journal::{Placement, PublishedFile},
        model::Provider,
        storage,
        store::{storage as objects, StoredObject},
    },
    model::InstanceId,
};
use std::path::PathBuf;
use uuid::Uuid;

struct Fixture {
    directory: tempfile::TempDir,
    db: Database,
    store: Arc<ObjectStore>,
    hook: Uuid,
}

async fn database(path: &Path) -> Database {
    let db = Database::init(&format!("sqlite://{}", path.display()))
        .await
        .unwrap();
    sqlx::query("INSERT INTO instances(id,name,dir,version,loader,manifest_url,meta_provider) VALUES(1,'gc','gc','1.21','vanilla','https://example.invalid','prism')").execute(&db.pool).await.unwrap();
    db
}

async fn fixture() -> Fixture {
    let directory = tempfile::tempdir().unwrap();
    let db = database(&directory.path().join("one.db")).await;
    let store = Arc::new(ObjectStore::new(directory.path().join("store")));
    let guard = store.try_maintenance().unwrap();
    attach(&db, &guard).await.unwrap(); // MUST precede the first managed bytes
    drop(guard);
    let hook = Uuid::new_v4();
    fs::write(directory.path().join("test-hook"), hook.to_string()).unwrap();
    Fixture {
        directory,
        db,
        store,
        hook,
    }
}

impl Fixture {
    async fn import(&self, body: &[u8]) -> (StoredObject, i64) {
        let source = self.directory.path().join("source");
        fs::write(&source, body).unwrap();
        let object = self
            .store
            .import_file(&source, None, &Cancellation::new())
            .unwrap();
        let id = objects::register(&self.db, &object).await.unwrap();
        (object, id)
    }

    async fn second(&self) -> Database {
        let db = database(&self.directory.path().join("two.db")).await;
        attach(&db, &self.store.try_maintenance().unwrap())
            .await
            .unwrap();
        db
    }
}

async fn bind(db: &Database, id: i64, name: &str, enabled: bool, pinned: bool, presence: &str) {
    sqlx::query("INSERT INTO instance_content(instance_id,object_id,kind,relative_path,path_key,mod_enabled,placement,pinned,presence) VALUES(1,?,'mod',?,? ,?,'copy',?,?)")
        .bind(id).bind(name).bind(name.as_bytes()).bind(enabled).bind(pinned).bind(presence)
        .execute(&db.pool).await.unwrap();
}

fn publication(object: &StoredObject, id: i64) -> PublishedFile {
    PublishedFile {
        name: "mod.jar".into(),
        sha256: object.digests.sha256.clone(),
        object_id: id,
        provider: Provider::Modrinth,
        project_id: "p".into(),
        release_id: "r".into(),
        placement: Placement::Copy,
        dependencies: None,
    }
}

async fn state(db: &Database, hash: &str) -> String {
    sqlx::query_scalar("SELECT store_state FROM content_objects WHERE sha256=?")
        .bind(hash)
        .fetch_one(&db.pool)
        .await
        .unwrap()
}

fn intent_path(f: &Fixture, object: &StoredObject) -> PathBuf {
    f.directory
        .path()
        .join("store")
        .join(INTENTS)
        .join(format!("{}.json", object.digests.sha256))
}

fn write_intent(f: &Fixture, object: &StoredObject) {
    let intent = DeleteIntent {
        version: 1,
        sha256: object.digests.sha256.clone(),
        sha1: object.digests.sha1.clone(),
        size: object.digests.size_bytes,
    };
    let path = intent_path(f, object);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, serde_json::to_vec(&intent).unwrap()).unwrap();
}

// Test-only faults are scoped by the physical fixture root, never global phase.
pub(super) async fn checkpoint(root: &Path, phase: &'static str) {
    if std::env::var_os("SONATA_TEST_GC_ROOT").as_deref() == Some(root.as_os_str())
        && std::env::var("SONATA_TEST_GC_PHASE").ok().as_deref() == Some(phase)
    {
        std::process::exit(74);
    }
    if let Ok(body) = fs::read_to_string(root.parent().unwrap().join("test-hook")) {
        if let Ok(id) = Uuid::parse_str(&body) {
            crate::instance::content::install::checkpoints::wait(id, phase).await;
        }
    }
}

#[async_std::test]
async fn collects_only_old_unbound_bytes_and_keeps_identity_provenance() {
    let f = fixture().await;
    let (object, id) = f.import(b"garbage").await;
    sqlx::query("INSERT INTO content_provider_files(object_id,provider,project_id,release_id,file_key,match_evidence) VALUES(?,'modrinth','p','r','file','exact')").bind(id).execute(&f.db.pool).await.unwrap();
    assert_eq!(
        collect(f.store.clone(), Duration::from_secs(7 * 86400))
            .await
            .unwrap(),
        0
    );
    assert!(object.path.is_file());
    assert_eq!(collect(f.store.clone(), Duration::ZERO).await.unwrap(), 1);
    assert!(!object.path.exists());
    assert_eq!(state(&f.db, &object.digests.sha256).await, "external");
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM content_provider_files WHERE object_id=?")
            .bind(id)
            .fetch_one(&f.db.pool)
            .await
            .unwrap();
    assert_eq!(count, 1);
    assert!(f
        .store
        .find_sha1(&object.digests.sha1, &Cancellation::new())
        .unwrap()
        .is_none());
    assert!(!intent_path(&f, &object).exists());
    assert_eq!(collect(f.store.clone(), Duration::ZERO).await.unwrap(), 0);
}

#[async_std::test]
async fn positive_retention_age_collects_old_files_but_preserves_future_timestamps() {
    let f = fixture().await;
    let (old, _) = f.import(b"old").await;
    let (future, _) = f.import(b"future").await;
    fs::File::open(&old.path)
        .unwrap()
        .set_times(
            fs::FileTimes::new()
                .set_modified(std::time::SystemTime::now() - Duration::from_secs(9 * 86400)),
        )
        .unwrap();
    fs::File::open(&future.path)
        .unwrap()
        .set_times(
            fs::FileTimes::new()
                .set_modified(std::time::SystemTime::now() + Duration::from_secs(86400)),
        )
        .unwrap();
    assert_eq!(
        collect(f.store.clone(), Duration::from_secs(7 * 86400))
            .await
            .unwrap(),
        1
    );
    assert!(!old.path.exists() && future.path.is_file());
}

#[async_std::test]
async fn missing_object_directory_replay_still_removes_only_the_matching_alias_and_repairs_sql() {
    let f = fixture().await;
    let (object, _) = f.import(b"removed shard").await;
    write_intent(&f, &object);
    // Fixture-specific target, not production cleanup: simulate lost CAS bytes.
    fs::remove_file(&object.path).unwrap();
    fs::remove_dir(object.path.parent().unwrap()).unwrap();
    assert_eq!(collect(f.store.clone(), Duration::ZERO).await.unwrap(), 0);
    assert_eq!(state(&f.db, &object.digests.sha256).await, "external");
    assert!(!intent_path(&f, &object).exists());
    let alias = f
        .directory
        .path()
        .join("store/aliases/sha1")
        .join(&object.digests.sha1);
    assert!(!alias.exists());
}

#[async_std::test]
async fn every_binding_is_a_root_regardless_of_enabled_pin_or_presence() {
    let f = fixture().await;
    let mut retained = Vec::new();
    for (i, presence) in ["present", "missing", "unstable", "unreadable"]
        .into_iter()
        .enumerate()
    {
        let (object, id) = f.import(format!("root-{i}").as_bytes()).await;
        bind(
            &f.db,
            id,
            &format!("mod-{i}.jar"),
            i % 2 == 0,
            i % 2 != 0,
            presence,
        )
        .await;
        retained.push(object);
    }
    let (garbage, _) = f.import(b"unused").await;
    assert_eq!(collect(f.store.clone(), Duration::ZERO).await.unwrap(), 1);
    assert!(!garbage.path.exists());
    for object in retained {
        assert!(object.path.is_file());
    }
}

#[async_std::test]
async fn preparation_without_object_ids_blocks_all_hashes_even_for_foreign_store_key() {
    let f = fixture().await;
    let (object, _) = f.import(b"pending unknown identity").await;
    storage::begin_preparation(&f.db, Uuid::new_v4(), InstanceId::from(1), b"foreign-root")
        .await
        .unwrap();
    assert!(collect(f.store.clone(), Duration::ZERO).await.is_err());
    assert!(object.path.is_file());
}

#[async_std::test]
async fn publishing_journals_and_leases_retain_content_until_abandon() {
    let f = fixture().await;
    let (object, id) = f.import(b"publication").await;
    let (extra, extra_id) = f.import(b"extra lease").await;
    let operation = Uuid::new_v4();
    storage::prepare(
        &f.db,
        operation,
        InstanceId::from(1),
        &[publication(&object, id)],
    )
    .await
    .unwrap();
    sqlx::query("INSERT INTO content_object_leases(operation_id,object_id) VALUES(?,?)")
        .bind(operation.to_string())
        .bind(extra_id)
        .execute(&f.db.pool)
        .await
        .unwrap();
    assert_eq!(collect(f.store.clone(), Duration::ZERO).await.unwrap(), 0);
    assert!(object.path.is_file() && extra.path.is_file());
    storage::abandon(&f.db, InstanceId::from(1), &operation.to_string())
        .await
        .unwrap();
    assert_eq!(collect(f.store.clone(), Duration::ZERO).await.unwrap(), 2);
}

#[async_std::test]
async fn malformed_unknown_or_inconsistent_publication_blocks_the_store() {
    for variant in [
        "corrupt",
        "future",
        "missing-lease",
        "wrong-hash",
        "unsafe-name",
        "duplicate",
        "bad-id",
    ] {
        let f = fixture().await;
        let (object, id) = f.import(b"publication").await;
        let (garbage, _) = f.import(b"not rooted").await;
        let operation = Uuid::new_v4();
        let mut file = publication(&object, id);
        storage::prepare(&f.db, operation, InstanceId::from(1), &[file.clone()])
            .await
            .unwrap();
        let body = match variant {
            "corrupt" => "{broken".to_owned(),
            "future" => "{\"version\":99,\"files\":[]}".to_owned(),
            "wrong-hash" => {
                file.sha256 = format!("{:064x}", 12);
                journal::encode(&[file]).unwrap()
            }
            "unsafe-name" => {
                file.name = "../outside.jar".into();
                journal::encode(&[file]).unwrap()
            }
            "duplicate" => journal::encode(&[file.clone(), file]).unwrap(),
            _ => journal::encode(&[file]).unwrap(),
        };
        sqlx::query("UPDATE content_operations SET recovery_json=? WHERE id=?")
            .bind(body)
            .bind(operation.to_string())
            .execute(&f.db.pool)
            .await
            .unwrap();
        if variant == "missing-lease" {
            sqlx::query("DELETE FROM content_object_leases")
                .execute(&f.db.pool)
                .await
                .unwrap();
        }
        if variant == "bad-id" {
            sqlx::query("DELETE FROM content_object_leases")
                .execute(&f.db.pool)
                .await
                .unwrap();
            sqlx::query("UPDATE content_operations SET id='bad' WHERE id=?")
                .bind(operation.to_string())
                .execute(&f.db.pool)
                .await
                .unwrap();
        }
        assert!(
            collect(f.store.clone(), Duration::ZERO).await.is_err(),
            "{variant}"
        );
        assert!(object.path.is_file() && garbage.path.is_file(), "{variant}");
    }
}

#[async_std::test]
async fn catalogued_databases_are_all_consulted_and_sql_metadata_is_repaired_in_each() {
    let f = fixture().await;
    let second = f.second().await;
    let (root, _) = f.import(b"owned by second").await;
    let id = objects::register(&second, &root).await.unwrap();
    bind(&second, id, "second.jar", false, false, "missing").await;
    let (garbage, _) = f.import(b"garbage in both databases").await;
    objects::register(&second, &garbage).await.unwrap();
    assert_eq!(collect(f.store.clone(), Duration::ZERO).await.unwrap(), 1);
    assert!(root.path.is_file());
    for db in [&f.db, &second] {
        assert_eq!(state(db, &garbage.digests.sha256).await, "external");
    }
}

#[async_std::test]
async fn missing_replaced_or_reused_database_path_blocks_deletion_without_creating_it() {
    for mode in ["missing", "replaced", "reattached"] {
        let f = fixture().await;
        let second = f.second().await;
        let path = owner(&second).await.unwrap().unwrap().path;
        second.pool.close().await;
        fs::rename(&path, f.directory.path().join("removed.db")).unwrap();
        let (object, _) = f.import(b"preserve when ownership lost").await;
        let replacement = if mode != "missing" {
            Some(database(&path).await)
        } else {
            None
        };
        if mode == "reattached" {
            attach(
                replacement.as_ref().unwrap(),
                &f.store.try_maintenance().unwrap(),
            )
            .await
            .unwrap();
        }
        assert!(
            collect(f.store.clone(), Duration::ZERO).await.is_err(),
            "{mode}"
        );
        assert!(object.path.is_file(), "{mode}");
        assert_eq!(path.exists(), mode != "missing");
    }
}

#[async_std::test]
async fn ephemeral_or_historical_ownership_cannot_become_complete_on_later_attach() {
    for mode in ["ephemeral", "historical", "owner-error"] {
        let f = fixture().await;
        if mode == "ephemeral" {
            let memory = Database::init("sqlite::memory:").await.unwrap();
            attach(&memory, &f.store.try_maintenance().unwrap())
                .await
                .unwrap();
        } else if mode == "owner-error" {
            let broken = Database {
                pool: sqlx::SqlitePool::connect(&format!(
                    "sqlite://{}?mode=rwc",
                    f.directory.path().join("broken.db").display()
                ))
                .await
                .unwrap(),
            };
            attach(&broken, &f.store.try_maintenance().unwrap())
                .await
                .unwrap();
        } else {
            fs::remove_file(f.directory.path().join("store/.sonata-roots.json")).unwrap();
            fs::create_dir_all(f.directory.path().join("store/objects")).unwrap();
        }
        attach(&f.db, &f.store.try_maintenance().unwrap())
            .await
            .unwrap();
        let (object, _) = f.import(b"unknown old roots").await;
        assert!(
            collect(f.store.clone(), Duration::ZERO).await.is_err(),
            "{mode}"
        );
        assert!(object.path.is_file());
    }
}

#[async_std::test]
async fn corrupt_catalogue_and_deletion_metadata_fail_closed() {
    for mode in [
        "catalogue-json",
        "catalogue-version",
        "intent-json",
        "intent-version",
        "intent-name",
        "intent-size",
    ] {
        let f = fixture().await;
        let (object, _) = f.import(b"preserve unknown metadata").await;
        let root = f.directory.path().join("store");
        match mode {
            "catalogue-json" => fs::write(root.join(".sonata-roots.json"), b"broken").unwrap(),
            "catalogue-version" => {
                let path = root.join(".sonata-roots.json");
                let mut json: serde_json::Value =
                    serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
                json["version"] = 99.into();
                fs::write(path, serde_json::to_vec(&json).unwrap()).unwrap();
            }
            _ => {
                write_intent(&f, &object);
                let path = intent_path(&f, &object);
                if mode == "intent-json" {
                    fs::write(path, b"broken").unwrap();
                } else if mode == "intent-name" {
                    fs::rename(path, root.join(INTENTS).join("other.json")).unwrap();
                } else {
                    let mut json: serde_json::Value =
                        serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
                    if mode == "intent-version" {
                        json["version"] = 99.into();
                    } else {
                        json["size"] = 1.into();
                    }
                    fs::write(path, serde_json::to_vec(&json).unwrap()).unwrap();
                }
            }
        }
        assert!(
            collect(f.store.clone(), Duration::ZERO).await.is_err(),
            "{mode}"
        );
        assert!(object.path.is_file(), "{mode}");
    }
}

#[async_std::test]
async fn replay_rechecks_new_roots_and_reimport_age() {
    let f = fixture().await;
    let (root, id) = f.import(b"new root since crash").await;
    write_intent(&f, &root);
    bind(&f.db, id, "new.jar", true, false, "present").await;
    let (fresh, _) = f.import(b"reimported bytes").await;
    write_intent(&f, &fresh);
    assert_eq!(
        collect(f.store.clone(), Duration::from_secs(86400))
            .await
            .unwrap(),
        0
    );
    assert!(root.path.is_file() && fresh.path.is_file());
    assert!(!intent_path(&f, &root).exists());
    assert!(intent_path(&f, &fresh).is_file());
    assert_eq!(collect(f.store.clone(), Duration::ZERO).await.unwrap(), 1);
    assert!(root.path.is_file() && !fresh.path.exists());
}

#[async_std::test]
async fn corrupt_objects_and_unknown_names_are_preserved() {
    let f = fixture().await;
    let (object, _) = f.import(b"original").await;
    fs::write(&object.path, b"corrupt").unwrap();
    let unknown = object.path.parent().unwrap().join("unknown-file");
    fs::write(&unknown, b"user bytes").unwrap();
    assert_eq!(collect(f.store.clone(), Duration::ZERO).await.unwrap(), 0);
    assert_eq!(fs::read(object.path).unwrap(), b"corrupt");
    assert_eq!(fs::read(unknown).unwrap(), b"user bytes");
}

#[async_std::test]
async fn nonmatching_sha1_alias_is_not_deleted() {
    let f = fixture().await;
    let (object, _) = f.import(b"old bytes").await;
    let alias = f
        .directory
        .path()
        .join("store/aliases/sha1")
        .join(&object.digests.sha1);
    let other = format!("{:064x}", 5);
    fs::write(&alias, &other).unwrap();
    assert_eq!(collect(f.store.clone(), Duration::ZERO).await.unwrap(), 1);
    assert_eq!(fs::read_to_string(alias).unwrap(), other);
}

#[cfg(unix)]
#[async_std::test]
async fn symlink_metadata_directories_and_database_aliases_are_conservative() {
    use std::os::unix::fs::symlink;
    for mode in [
        "catalogue",
        "intents",
        "objects",
        "shard",
        "target",
        "database",
    ] {
        let f = fixture().await;
        let (object, _) = f.import(b"linked target").await;
        let root = f.directory.path().join("store");
        let target = match mode {
            "catalogue" => root.join(".sonata-roots.json"),
            "intents" => {
                write_intent(&f, &object);
                root.join(INTENTS)
            }
            "objects" => root.join("objects"),
            "shard" => object.path.parent().unwrap().to_owned(),
            "target" => object.path.clone(),
            _ => {
                f.db.pool.close().await;
                f.directory.path().join("one.db")
            }
        };
        let outside = f.directory.path().join("outside");
        fs::rename(&target, &outside).unwrap();
        symlink(&outside, &target).unwrap();
        let result = collect(f.store.clone(), Duration::ZERO).await;
        if matches!(mode, "shard" | "target") {
            assert_eq!(result.unwrap(), 0);
        } else {
            assert!(result.is_err(), "{mode}");
        }
        assert!(object.path.is_file(), "{mode}");
        assert!(outside.exists(), "{mode}");
    }
}

#[cfg(unix)]
#[async_std::test]
async fn canonical_store_alias_attaches_to_the_same_catalogue() {
    let f = fixture().await;
    let alias = f.directory.path().join("store-alias");
    std::os::unix::fs::symlink(f.directory.path().join("store"), &alias).unwrap();
    let store = Arc::new(ObjectStore::new(alias));
    attach(&f.db, &store.try_maintenance().unwrap())
        .await
        .unwrap();
    assert_eq!(
        RootCatalog::owners(&store.try_maintenance().unwrap())
            .unwrap()
            .len(),
        1
    );
    let (object, _) = f.import(b"alias gc").await;
    assert_eq!(collect(store, Duration::ZERO).await.unwrap(), 1);
    assert!(!object.path.exists());
}

#[async_std::test]
async fn caller_drop_keeps_maintenance_and_sql_snapshot_until_owned_collection_finishes() {
    let f = fixture().await;
    let (object, _) = f.import(b"owned collector").await;
    let mut pause = Pause::new(f.hook, "snapshot");
    let mut future = Box::pin(collect(f.store.clone(), Duration::ZERO));
    futures::select! {
        _ = futures::FutureExt::fuse(pause.entered()) => {},
        result = futures::FutureExt::fuse(future.as_mut()) => panic!("collection completed early: {result:?}"),
    }
    drop(future);
    assert!(f.store.try_access().is_err());
    let probe = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(
            SqliteConnectOptions::new()
                .filename(f.directory.path().join("one.db"))
                .busy_timeout(Duration::ZERO),
        )
        .await
        .unwrap();
    let error = probe
        .begin_with("BEGIN IMMEDIATE")
        .await
        .err()
        .expect("snapshot lost its SQL lock");
    assert!(error
        .as_database_error()
        .is_some_and(|error| error.message().contains("locked")));
    probe.close().await;
    let mut finished = Pause::new(f.hook, "cleaned");
    pause.resume();
    finished.entered().await;
    assert!(!object.path.exists());
    assert_eq!(state(&f.db, &object.digests.sha256).await, "external");
    finished.resume();
    f.store.access(&Cancellation::new()).await.unwrap();
}

#[test]
fn crash_collector() {
    let Some(root) = std::env::var_os("SONATA_TEST_GC_ROOT").map(PathBuf::from) else {
        return;
    };
    async_std::task::block_on(async {
        collect(Arc::new(ObjectStore::new(root)), Duration::ZERO)
            .await
            .unwrap();
        panic!("missed GC crash checkpoint");
    });
}

async fn crash(f: &Fixture, phase: &str) {
    let root = f.directory.path().join("store");
    let phase = phase.to_owned();
    let output = async_std::task::spawn_blocking(move || {
        std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "instance::content::retention::tests::crash_collector",
                "--nocapture",
            ])
            .env("SONATA_TEST_GC_ROOT", root)
            .env("SONATA_TEST_GC_PHASE", phase)
            .output()
            .unwrap()
    })
    .await;
    assert_eq!(
        output.status.code(),
        Some(74),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[async_std::test]
async fn process_exit_replays_intent_unlink_alias_and_each_sql_commit_boundary() {
    for phase in [
        "intent",
        "unlink",
        "alias",
        "sql",
        "first-commit",
        "cleaned",
    ] {
        let f = fixture().await;
        let second = f.second().await;
        let (object, _) = f.import(b"crash garbage").await;
        objects::register(&second, &object).await.unwrap();
        crash(&f, phase).await;
        assert_eq!(object.path.exists(), phase == "intent", "{phase}");
        assert_eq!(
            intent_path(&f, &object).exists(),
            phase != "cleaned",
            "{phase}"
        );
        assert_eq!(
            collect(f.store.clone(), Duration::ZERO).await.unwrap(),
            usize::from(phase == "intent"),
            "{phase}"
        );
        for db in [&f.db, &second] {
            assert_eq!(
                state(db, &object.digests.sha256).await,
                "external",
                "{phase}"
            );
        }
        assert!(!intent_path(&f, &object).exists());
        assert!(f
            .store
            .find_sha1(&object.digests.sha1, &Cancellation::new())
            .unwrap()
            .is_none());
        assert_eq!(collect(f.store.clone(), Duration::ZERO).await.unwrap(), 0);
    }
}

#[async_std::test]
async fn process_exit_intent_replay_preserves_a_root_added_after_crash() {
    let f = fixture().await;
    let (object, id) = f.import(b"became live").await;
    crash(&f, "intent").await;
    bind(&f.db, id, "live.jar", true, false, "present").await;
    assert_eq!(collect(f.store.clone(), Duration::ZERO).await.unwrap(), 0);
    assert!(object.path.is_file());
    assert!(!intent_path(&f, &object).exists());
}
