use super::*;
use crate::utils::object_store::{
    collection::{DeleteIntent, INTENTS},
    StoredObject,
};

struct Fixture {
    directory: tempfile::TempDir,
    assets: PathBuf,
    db: Arc<Database>,
    store: Arc<ObjectStore>,
}

async fn database(path: &Path) -> Arc<Database> {
    Arc::new(
        Database::init(&format!("sqlite://{}", path.display()))
            .await
            .unwrap(),
    )
}

async fn fixture() -> Fixture {
    let directory = tempfile::tempdir().unwrap();
    let assets = directory.path().join("assets");
    let db = database(&directory.path().join("assets.db")).await;
    attach(db.clone(), assets.join("objects"), Cancellation::new())
        .await
        .unwrap();
    let store = Arc::new(ObjectStore::new(assets.join("objects/.sonata-cas")));
    Fixture {
        directory,
        assets,
        db,
        store,
    }
}

impl Fixture {
    fn import(&self, bytes: &[u8]) -> StoredObject {
        let source = self.directory.path().join("source");
        fs::write(&source, bytes).unwrap();
        self.store
            .import_file(&source, None, &Cancellation::new())
            .unwrap()
    }
    async fn root(&self, hash: &str) {
        sqlx::query("INSERT INTO assets(name,hash,url) VALUES('root',?,'unused')")
            .bind(hash)
            .execute(&self.db.pool)
            .await
            .unwrap();
    }
    fn placement(&self, object: &StoredObject, bytes: &[u8]) -> PathBuf {
        let path = self
            .assets
            .join("objects")
            .join(&object.digests.sha1[..2])
            .join(&object.digests.sha1);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, bytes).unwrap();
        path
    }
    fn intent(&self, object: &StoredObject) -> PathBuf {
        self.store
            .root()
            .join(INTENTS)
            .join(format!("{}.json", object.digests.sha256))
    }
}

#[async_std::test]
async fn asset_collection_keeps_all_sql_index_and_placement_roots_without_relying_on_aliases() {
    let f = fixture().await;
    let sql = f.import(b"SQL root missing placement");
    f.root(&sql.digests.sha1).await;
    let index = f.import(b"index root missing placement");
    fs::create_dir_all(f.assets.join("indexes")).unwrap();
    fs::write(
        f.assets.join("indexes/url.json"),
        serde_json::to_vec(&serde_json::json!({"objects": {"a": {"hash": index.digests.sha1}}}))
            .unwrap(),
    )
    .unwrap();
    let placement = f.import(b"placement root");
    let path = f.placement(&placement, b"corrupt placement needs repair");
    let garbage = f.import(b"unused CAS bytes");
    fs::write(
        f.store.root().join("aliases/sha1").join(&sql.digests.sha1),
        b"corrupt hint",
    )
    .unwrap();
    assert_eq!(collect(f.assets.clone(), Duration::ZERO).await.unwrap(), 1);
    assert!(sql.path.is_file() && index.path.is_file() && placement.path.is_file());
    assert!(!garbage.path.exists());
    assert_eq!(fs::read(path).unwrap(), b"corrupt placement needs repair");
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM assets")
        .fetch_one(&f.db.pool)
        .await
        .unwrap();
    assert_eq!(count, 1);
}

#[async_std::test]
async fn all_asset_databases_and_their_identities_are_required_not_only_the_current_owner() {
    for mode in ["root", "missing", "replaced", "invalid-hash"] {
        let f = fixture().await;
        let object = f.import(b"other owner input");
        let path = f.directory.path().join("other.db");
        let db = database(&path).await;
        attach(db.clone(), f.assets.join("objects"), Cancellation::new())
            .await
            .unwrap();
        let hash = if mode == "invalid-hash" {
            "zz".repeat(20)
        } else {
            object.digests.sha1.clone()
        };
        sqlx::query("INSERT INTO assets(name,hash,url) VALUES('other',?,'unused')")
            .bind(hash)
            .execute(&db.pool)
            .await
            .unwrap();
        if mode == "replaced" {
            sqlx::query("DROP TRIGGER retention_identity_update")
                .execute(&db.pool)
                .await
                .unwrap();
            sqlx::query("UPDATE retention_identity SET identity=?")
                .bind("ab".repeat(16))
                .execute(&db.pool)
                .await
                .unwrap();
        }
        if mode == "missing" {
            db.pool.close().await;
            fs::rename(path, f.directory.path().join("moved.db")).unwrap();
        }
        if mode == "root" {
            assert_eq!(collect(f.assets.clone(), Duration::ZERO).await.unwrap(), 0);
        } else {
            assert!(
                collect(f.assets.clone(), Duration::ZERO).await.is_err(),
                "{mode}"
            );
        }
        assert!(object.path.is_file() && !f.intent(&object).exists());
    }
}

#[async_std::test]
async fn unknown_old_or_ephemeral_asset_owners_disable_collection() {
    for mode in ["historical", "ephemeral", "catalogue-corrupt"] {
        let f = fixture().await;
        let object = f.import(b"unknown ownership");
        if mode == "historical" {
            fs::remove_file(f.store.root().join(".sonata-roots.json")).unwrap();
        }
        if mode == "ephemeral" {
            let db = Arc::new(Database::init("sqlite::memory:").await.unwrap());
            attach(db, f.assets.join("objects"), Cancellation::new())
                .await
                .unwrap();
        } else if mode == "catalogue-corrupt" {
            fs::write(f.store.root().join(".sonata-roots.json"), b"broken").unwrap();
        } else {
            attach(f.db.clone(), f.assets.join("objects"), Cancellation::new())
                .await
                .unwrap();
        }
        assert!(
            collect(f.assets.clone(), Duration::ZERO).await.is_err(),
            "{mode}"
        );
        assert!(object.path.is_file());
    }
}

#[async_std::test]
async fn unknown_layouts_and_corrupt_asset_indexes_defer_all_collection_before_intents() {
    for mode in [
        "index",
        "index-hash",
        "virtual",
        "shard",
        "placement",
        "uppercase",
        "cas-metadata",
        "pending-import",
        "future-index",
        "future-object",
    ] {
        let f = fixture().await;
        let object = f.import(b"unknown inputs");
        match mode {
            "cas-metadata" => {
                fs::write(f.store.root().join("future-preparations"), b"unknown").unwrap()
            }
            "pending-import" => {
                fs::create_dir_all(f.store.root().join("staging/import-unknown")).unwrap();
            }
            "future-index" | "future-object" => {
                fs::create_dir(f.assets.join("indexes")).unwrap();
                let mut index = serde_json::json!({"objects":{"a":{"hash":object.digests.sha1}}});
                if mode == "future-index" {
                    index["additional_roots"] = true.into();
                } else {
                    index["objects"]["a"]["additional_input"] = true.into();
                }
                fs::write(
                    f.assets.join("indexes/future.json"),
                    serde_json::to_vec(&index).unwrap(),
                )
                .unwrap();
            }
            "index" | "index-hash" => {
                fs::create_dir(f.assets.join("indexes")).unwrap();
                fs::write(
                    f.assets.join("indexes/index.json"),
                    if mode == "index" {
                        b"broken".as_slice()
                    } else {
                        br#"{"objects":{"a":{"hash":"bad"}}}"#.as_slice()
                    },
                )
                .unwrap();
            }
            "virtual" => fs::create_dir(f.assets.join("virtual")).unwrap(),
            "shard" => fs::create_dir(f.assets.join("objects/not-a-hash")).unwrap(),
            "uppercase" => fs::create_dir(f.assets.join("objects/AB")).unwrap(),
            _ => {
                fs::create_dir(f.assets.join("objects/ab")).unwrap();
                fs::write(f.assets.join("objects/ab/unknown"), b"data").unwrap();
            }
        }
        assert!(
            collect(f.assets.clone(), Duration::ZERO).await.is_err(),
            "{mode}"
        );
        assert!(object.path.is_file() && !f.intent(&object).exists());
    }
}

#[async_std::test]
async fn admission_excludes_asset_importers_and_age_keeps_recent_or_future_bytes() {
    let f = fixture().await;
    let object = f.import(b"young bytes");
    let access = f.store.try_access().unwrap();
    assert!(collect(f.assets.clone(), Duration::ZERO).await.is_err());
    drop(access);
    assert_eq!(
        collect(f.assets.clone(), Duration::from_secs(7 * 86400))
            .await
            .unwrap(),
        0
    );
    fs::File::open(&object.path)
        .unwrap()
        .set_times(
            fs::FileTimes::new()
                .set_modified(std::time::SystemTime::now() + Duration::from_secs(86400)),
        )
        .unwrap();
    assert_eq!(collect(f.assets.clone(), Duration::ZERO).await.unwrap(), 0);
    assert!(object.path.is_file());
}

type Hook = (async_channel::Sender<()>, std::sync::mpsc::Receiver<()>);
static HOOKS: std::sync::OnceLock<
    std::sync::Mutex<std::collections::HashMap<(PathBuf, &'static str), Hook>>,
> = std::sync::OnceLock::new();
pub(super) fn checkpoint(root: &Path, phase: &'static str) {
    let hook = HOOKS
        .get_or_init(Default::default)
        .lock()
        .unwrap()
        .remove(&(root.to_owned(), phase));
    if let Some((entered, resume)) = hook {
        entered.send_blocking(()).unwrap();
        resume.recv_timeout(Duration::from_secs(10)).unwrap();
    }
    if std::env::var_os("SONATA_TEST_ASSET_GC_ROOT")
        .is_some_and(|value| Path::new(&value).join("assets/objects/.sonata-cas") == root)
        && std::env::var("SONATA_TEST_ASSET_GC_PHASE").ok().as_deref() == Some(phase)
    {
        std::process::exit(77);
    }
}

type SnapshotHook = (
    futures::channel::oneshot::Sender<()>,
    futures::channel::oneshot::Receiver<()>,
);
static SNAPSHOTS: std::sync::OnceLock<
    std::sync::Mutex<std::collections::HashMap<PathBuf, SnapshotHook>>,
> = std::sync::OnceLock::new();
pub(super) async fn snapshot_checkpoint(root: &Path) {
    let hook = SNAPSHOTS
        .get_or_init(Default::default)
        .lock()
        .unwrap()
        .remove(root);
    if let Some((entered, resume)) = hook {
        let _ = entered.send(());
        let _ = resume.await;
    }
}

#[async_std::test]
async fn asset_crash_collector() {
    let Some(root) = std::env::var_os("SONATA_TEST_ASSET_GC_ROOT").map(PathBuf::from) else {
        return;
    };
    collect(root.join("assets"), Duration::ZERO).await.unwrap();
    panic!("missed asset deletion crash boundary");
}

async fn crash(f: &Fixture, phase: &str) {
    let root = f.directory.path().to_owned();
    let phase = phase.to_owned();
    let output = async_std::task::spawn_blocking(move || {
        std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "instance::download::assets::retention::tests::asset_crash_collector",
                "--nocapture",
            ])
            .env("SONATA_TEST_ASSET_GC_ROOT", root)
            .env("SONATA_TEST_ASSET_GC_PHASE", phase)
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
}

#[async_std::test]
async fn process_exit_replays_every_asset_cas_deletion_boundary_idempotently() {
    for phase in ["intent", "unlink", "alias", "cleaned"] {
        let f = fixture().await;
        let object = f.import(b"unrooted asset CAS");
        crash(&f, phase).await;
        collect(f.assets.clone(), Duration::ZERO).await.unwrap();
        assert!(
            !object.path.exists() && !f.intent(&object).exists(),
            "{phase}"
        );
        assert_eq!(collect(f.assets.clone(), Duration::ZERO).await.unwrap(), 0);
    }
}

#[async_std::test]
async fn asset_replay_rechecks_new_sql_index_and_placement_roots_and_reimport_age() {
    for mode in ["sql", "index", "placement", "reimport"] {
        let f = fixture().await;
        let object = f.import(b"new retention root");
        crash(&f, "intent").await;
        match mode {
            "sql" => f.root(&object.digests.sha1).await,
            "index" => {
                fs::create_dir(f.assets.join("indexes")).unwrap();
                fs::write(
                    f.assets.join("indexes/new.json"),
                    serde_json::to_vec(
                        &serde_json::json!({"objects":{"a":{"hash":object.digests.sha1}}}),
                    )
                    .unwrap(),
                )
                .unwrap();
            }
            "placement" => {
                f.placement(&object, b"intended path root");
            }
            _ => {
                fs::remove_file(&object.path).unwrap();
                let new = f.import(b"new retention root");
                assert_eq!(new.path, object.path);
            }
        }
        assert_eq!(
            collect(
                f.assets.clone(),
                if mode == "reimport" {
                    Duration::from_secs(86400)
                } else {
                    Duration::ZERO
                }
            )
            .await
            .unwrap(),
            0
        );
        assert!(object.path.is_file());
        if mode != "reimport" {
            assert!(!f.intent(&object).exists());
        }
    }
}

#[async_std::test]
async fn corrupt_or_unknown_asset_deletion_intents_keep_bytes() {
    let f = fixture().await;
    let object = f.import(b"corrupt replay metadata");
    fs::create_dir_all(f.store.root().join(INTENTS)).unwrap();
    fs::write(f.intent(&object), b"broken").unwrap();
    assert!(collect(f.assets.clone(), Duration::ZERO).await.is_err());
    assert!(object.path.is_file());
    let intent = DeleteIntent {
        version: 99,
        sha256: object.digests.sha256.clone(),
        sha1: object.digests.sha1.clone(),
        size: object.digests.size_bytes,
    };
    fs::write(f.intent(&object), serde_json::to_vec(&intent).unwrap()).unwrap();
    assert!(collect(f.assets.clone(), Duration::ZERO).await.is_err());
    assert!(object.path.is_file());
}

#[async_std::test]
async fn dropping_asset_collector_caller_keeps_sql_and_admission_through_blocking_unlink() {
    for phase in ["snapshot", "intent"] {
        let f = fixture().await;
        let object = f.import(b"owned collector");
        let (entered, receiver) = futures::channel::oneshot::channel();
        let (resume, wait) = futures::channel::oneshot::channel();
        let (blocking_entered, blocking_receiver) = async_channel::bounded(1);
        let (blocking_resume, blocking_wait) = std::sync::mpsc::channel();
        if phase == "snapshot" {
            SNAPSHOTS
                .get_or_init(Default::default)
                .lock()
                .unwrap()
                .insert(f.store.root().to_owned(), (entered, wait));
        } else {
            HOOKS.get_or_init(Default::default).lock().unwrap().insert(
                (f.store.root().to_owned(), "intent"),
                (blocking_entered, blocking_wait),
            );
        }
        let gate = async move {
            if phase == "snapshot" {
                receiver.await.unwrap();
            } else {
                blocking_receiver.recv().await.unwrap();
            }
        };
        let mut caller = Box::pin(collect(f.assets.clone(), Duration::ZERO));
        match futures::future::select(Box::pin(gate), caller.as_mut()).await {
            futures::future::Either::Left(((), _)) => {}
            futures::future::Either::Right((result, _)) => panic!("finished early: {result:?}"),
        };
        drop(caller);
        assert!(f.store.try_access().is_err());
        let options: sqlx::sqlite::SqliteConnectOptions = format!(
            "sqlite://{}",
            f.directory.path().join("assets.db").display()
        )
        .parse()
        .unwrap();
        let probe = sqlx::SqlitePool::connect_with(options.busy_timeout(Duration::from_millis(20)))
            .await
            .unwrap();
        assert!(
            sqlx::query("INSERT INTO assets(name,hash,url) VALUES('probe',?,'unused')")
                .bind("ab".repeat(20))
                .execute(&probe)
                .await
                .is_err()
        );
        if phase == "snapshot" {
            resume.send(()).unwrap();
        } else {
            blocking_resume.send(()).unwrap();
        }
        let maintenance = f.store.maintenance(&Cancellation::new()).await.unwrap();
        drop(maintenance);
        sqlx::query("INSERT INTO assets(name,hash,url) VALUES('probe',?,'unused')")
            .bind("ab".repeat(20))
            .execute(&probe)
            .await
            .unwrap();
        assert!(!object.path.exists() && !f.intent(&object).exists());
        probe.close().await;
    }
}

#[cfg(unix)]
#[async_std::test]
async fn canonical_asset_aliases_share_admission_and_links_defer_collection() {
    let f = fixture().await;
    let object = f.import(b"canonical asset store");
    let alias = f.directory.path().join("alias");
    std::os::unix::fs::symlink(&f.assets, &alias).unwrap();
    let access = f.store.try_access().unwrap();
    assert!(collect(alias.clone(), Duration::ZERO).await.is_err());
    drop(access);
    let path = f.placement(&object, b"linked intended root");
    fs::remove_file(&path).unwrap();
    std::os::unix::fs::symlink(&object.path, &path).unwrap();
    assert!(collect(alias.clone(), Duration::ZERO).await.is_err());
    assert!(object.path.is_file());
    fs::remove_file(path).unwrap();
    assert_eq!(collect(alias, Duration::ZERO).await.unwrap(), 1);
}

#[async_std::test]
async fn startup_asset_collection_keeps_current_and_recent_bytes_and_is_idempotent() {
    let f = fixture().await;
    let rooted = f.import(b"SQL asset root");
    f.root(&rooted.digests.sha1).await;
    let garbage = f.import(b"old orphan");
    let recent = f.import(b"recent orphan");
    fs::File::open(&garbage.path)
        .unwrap()
        .set_times(
            fs::FileTimes::new()
                .set_modified(std::time::SystemTime::now() - Duration::from_secs(9 * 86400)),
        )
        .unwrap();
    super::super::AssetsData::initialize(f.db.clone(), f.assets.clone())
        .await
        .unwrap();
    assert!(!garbage.path.exists() && rooted.path.is_file() && recent.path.is_file());
    super::super::AssetsData::initialize(f.db.clone(), f.assets.clone())
        .await
        .unwrap();
    assert!(rooted.path.is_file() && recent.path.is_file());
}
