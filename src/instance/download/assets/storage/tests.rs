use super::*;
use crate::instance::download::assets::parse;
use serde_json::json;
use std::fs;

type Hook = (async_channel::Sender<()>, std::sync::mpsc::Receiver<()>);
static HOOKS: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<PathBuf, Hook>>> =
    std::sync::OnceLock::new();

pub(super) fn checkpoint(root: &Path, phase: &str) {
    if phase == "admitted" {
        let hook = HOOKS
            .get_or_init(Default::default)
            .lock()
            .unwrap()
            .remove(root);
        if let Some((entered, resume)) = hook {
            entered.send_blocking(()).unwrap();
            resume
                .recv_timeout(std::time::Duration::from_secs(10))
                .unwrap();
        }
    }
    if phase == "imported"
        && std::env::var_os("SONATA_TEST_ASSET_ROOT")
            .is_some_and(|value| Path::new(&value).join("objects/.sonata-cas") == root)
    {
        std::process::exit(77);
    }
}

async fn fixture() -> (
    tempfile::TempDir,
    Arc<Database>,
    DbAssetStore,
    Vec<AssetInfo>,
) {
    let directory = tempfile::tempdir().unwrap();
    let db = Arc::new(Database::init("sqlite::memory:").await.unwrap());
    let objects = directory.path().join("objects");
    let hash = format!("{:x}", <sha1::Sha1 as sha1::Digest>::digest(b"asset bytes"));
    let wanted = parse::extract_assets(&json!({"objects": {"a": {"hash": hash}}})).unwrap();
    let store = DbAssetStore::new(db.clone(), objects, Cancellation::new());
    (directory, db, store, wanted)
}

fn placement(store: &DbAssetStore, asset: &AssetInfo) -> PathBuf {
    let path = asset.object_path(&store.objects);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, b"asset bytes").unwrap();
    path
}

#[async_std::test]
async fn legacy_bytes_import_without_network_or_identity_changes_and_missing_placements_are_copies()
{
    let (_directory, db, store, wanted) = fixture().await;
    let path = placement(&store, &wanted[0]);
    store.register(&wanted).await.unwrap();
    let row: (i64, String, String, String) = sqlx::query_as("SELECT id,hash,name,url FROM assets")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    let cas = ObjectStore::new(store.objects.join(".sonata-cas"));
    let object = cas
        .find_sha1(wanted[0].hash(), &Cancellation::new())
        .unwrap()
        .unwrap();
    assert_eq!(object.digests.sha256, files::sha256(b"asset bytes"));
    assert_eq!(fs::read(&path).unwrap(), b"asset bytes");
    fs::remove_file(&path).unwrap();
    assert_eq!(store.cached(&wanted).await.unwrap().len(), 1);
    fs::write(&path, b"modified placement").unwrap();
    assert_eq!(fs::read(&object.path).unwrap(), b"asset bytes");
    assert_eq!(store.cached(&wanted).await.unwrap().len(), 1);
    assert_eq!(fs::read(path).unwrap(), b"asset bytes");
    let after =
        sqlx::query_as::<_, (i64, String, String, String)>("SELECT id,hash,name,url FROM assets")
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(row, after);
}

#[async_std::test]
async fn asset_cas_aliases_follow_relocation_and_identical_names_do_not_change_byte_identity() {
    let (directory, db, store, mut wanted) = fixture().await;
    placement(&store, &wanted[0]);
    let mut second = wanted[0].clone();
    second.name = "another-role".into();
    wanted.push(second);
    store.register(&wanted).await.unwrap();
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM assets")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(count, 1);
    let moved = directory.path().join("moved");
    fs::rename(&store.objects, &moved).unwrap();
    let relocated = DbAssetStore::new(db, moved, Cancellation::new());
    fs::remove_file(wanted[0].object_path(&relocated.objects)).unwrap();
    assert_eq!(relocated.cached(&wanted).await.unwrap().len(), 1);
    assert_eq!(
        fs::read(wanted[0].object_path(&relocated.objects)).unwrap(),
        b"asset bytes"
    );
}

#[async_std::test]
async fn sql_and_alias_hints_never_substitute_bad_bytes_and_corrupt_cas_is_not_overwritten() {
    let (_directory, db, store, wanted) = fixture().await;
    let path = placement(&store, &wanted[0]);
    store.register(&wanted).await.unwrap();
    let cas = ObjectStore::new(store.objects.join(".sonata-cas"));
    let object = cas
        .find_sha1(wanted[0].hash(), &Cancellation::new())
        .unwrap()
        .unwrap();
    fs::write(&object.path, b"corrupt authoritative bytes").unwrap();
    fs::remove_file(&path).unwrap();
    assert!(store.cached(&wanted).await.unwrap().is_empty());
    assert!(store.register(&wanted).await.is_err());
    placement(&store, &wanted[0]);
    assert!(store.cached(&wanted).await.is_err());
    assert_eq!(
        fs::read(object.path).unwrap(),
        b"corrupt authoritative bytes"
    );
    assert_eq!(fs::read(path).unwrap(), b"asset bytes");
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM assets")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(count, 1);
}

#[async_std::test]
async fn registration_requires_verified_downloads_before_sql_and_admission_wait_is_cancellable() {
    let (_directory, db, store, wanted) = fixture().await;
    assert!(store.register(&wanted).await.is_err());
    let path = placement(&store, &wanted[0]);
    fs::write(&path, b"wrong hash").unwrap();
    assert!(store.register(&wanted).await.is_err());
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM assets")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
    let cas = ObjectStore::new(store.objects.join(".sonata-cas"));
    let _maintenance = cas.try_maintenance().unwrap();
    let trigger = store.cancel.clone();
    let (result, ()) = futures::join!(store.cached(&wanted), async {
        async_std::task::yield_now().await;
        trigger.cancel();
    });
    assert!(result.is_err());
    assert_eq!(fs::read(path).unwrap(), b"wrong hash");
    assert!(!cas.root().join("objects").exists());
}

#[cfg(unix)]
#[async_std::test]
async fn linked_or_escaping_asset_placements_never_modify_external_bytes() {
    for mode in ["file", "ancestor", "cas-root"] {
        let (directory, _db, store, wanted) = fixture().await;
        let outside = directory.path().join("outside");
        fs::create_dir(&outside).unwrap();
        let victim = outside.join("victim");
        fs::write(&victim, b"user bytes").unwrap();
        let path = wanted[0].object_path(&store.objects);
        fs::create_dir_all(&store.objects).unwrap();
        match mode {
            "file" => {
                fs::create_dir_all(path.parent().unwrap()).unwrap();
                std::os::unix::fs::symlink(&victim, &path).unwrap();
            }
            "ancestor" => std::os::unix::fs::symlink(&outside, path.parent().unwrap()).unwrap(),
            _ => std::os::unix::fs::symlink(&outside, store.objects.join(".sonata-cas")).unwrap(),
        }
        assert!(store.cached(&wanted).await.is_err(), "{mode}");
        assert!(store.register(&wanted).await.is_err(), "{mode}");
        assert_eq!(fs::read(victim).unwrap(), b"user bytes");
        assert_eq!(fs::read_dir(outside).unwrap().count(), 1);
    }
}

#[async_std::test]
async fn dropping_the_asset_cache_caller_keeps_worker_admission_until_import_completes() {
    let (_directory, _db, store, wanted) = fixture().await;
    let path = placement(&store, &wanted[0]);
    let cas = ObjectStore::new(store.objects.join(".sonata-cas"));
    let access = cas.try_access().unwrap();
    drop(access);
    let (entered, receiver) = async_channel::bounded(1);
    let (resume, wait) = std::sync::mpsc::channel();
    HOOKS
        .get_or_init(Default::default)
        .lock()
        .unwrap()
        .insert(cas.root().to_owned(), (entered, wait));
    let mut caller = Box::pin(store.cached(&wanted));
    match futures::future::select(Box::pin(receiver.recv()), caller.as_mut()).await {
        futures::future::Either::Left((result, _)) => result.unwrap(),
        futures::future::Either::Right((result, _)) => panic!("worker finished early: {result:?}"),
    }
    drop(caller);
    assert!(cas.try_maintenance().is_err());
    resume.send(()).unwrap();
    let maintenance = cas.maintenance(&Cancellation::new()).await.unwrap();
    drop(maintenance);
    assert!(cas
        .find_sha1(wanted[0].hash(), &Cancellation::new())
        .unwrap()
        .is_some());
    assert_eq!(fs::read(path).unwrap(), b"asset bytes");
}

#[async_std::test]
async fn asset_import_crash_writer() {
    let Some(root) = std::env::var_os("SONATA_TEST_ASSET_ROOT").map(PathBuf::from) else {
        return;
    };
    let db = Arc::new(
        Database::init(&format!("sqlite://{}", root.join("asset.db").display()))
            .await
            .unwrap(),
    );
    let hash = format!("{:x}", <sha1::Sha1 as sha1::Digest>::digest(b"asset bytes"));
    let wanted = parse::extract_assets(&json!({"objects": {"a": {"hash": hash}}})).unwrap();
    let store = DbAssetStore::new(db, root.join("objects"), Cancellation::new());
    store.register(&wanted).await.unwrap();
    panic!("missed asset import crash boundary");
}

#[async_std::test]
async fn process_exit_between_asset_import_and_sql_preserves_bytes_and_replays_without_duplicate_identity(
) {
    let (directory, _db, store, wanted) = fixture().await;
    let db = Arc::new(
        Database::init(&format!(
            "sqlite://{}",
            directory.path().join("asset.db").display()
        ))
        .await
        .unwrap(),
    );
    let path = placement(&store, &wanted[0]);
    let root = directory.path().to_owned();
    let output = async_std::task::spawn_blocking(move || {
        std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "instance::download::assets::storage::tests::asset_import_crash_writer",
                "--nocapture",
            ])
            .env("SONATA_TEST_ASSET_ROOT", root)
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
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM assets")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
    let restarted = DbAssetStore::new(db.clone(), store.objects.clone(), Cancellation::new());
    restarted.register(&wanted).await.unwrap();
    restarted.register(&wanted).await.unwrap();
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM assets")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(count, 1);
    assert_eq!(fs::read(path).unwrap(), b"asset bytes");
}

#[async_std::test]
async fn asset_sync_repairs_missing_placement_without_transport_and_preserves_minecraft_layout() {
    let (directory, db, store, wanted) = fixture().await;
    let path = placement(&store, &wanted[0]);
    store.register(&wanted).await.unwrap();
    fs::remove_file(&path).unwrap();
    let paths = crate::data::layout::LauncherPaths::resolve(&crate::data::config::Config {
        root_override: Some(directory.path().to_owned()),
        dirs: crate::data::config::DirOverrides {
            assets: Some(directory.path().to_owned()),
            ..Default::default()
        },
        ..crate::data::config::Config::init()
    })
    .await
    .unwrap();
    let operations =
        crate::data::registry::operation::OperationRegistry::new(crate::bus::EventBus::new(16));
    let op = operations.begin("assets".into(), Vec::new());
    let manifest = json!({"objects": {"a": {"hash": wanted[0].hash()}}});
    let result = crate::instance::download::assets::AssetsData::sync(&manifest, &paths, db, &op)
        .await
        .unwrap();
    assert_eq!((result.total, result.downloaded), (1, 0));
    assert_eq!(fs::read(path).unwrap(), b"asset bytes");
    op.finish(crate::data::registry::operation::message::status::Outcome::Completed);
}
