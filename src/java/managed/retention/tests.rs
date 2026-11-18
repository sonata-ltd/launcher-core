use super::*;
use crate::java::{
    managed::{
        tests::{package, pin_archive, tar_fixture},
        ManagedJavaService, ManagedJavaStage,
    },
    model::{JavaSource, NewJavaRuntime},
    storage::{DbJavaStore, JavaStore},
};
use crate::utils::cancellation::Cancellation;
use std::time::Duration;

type Hook = (
    futures::channel::oneshot::Sender<()>,
    futures::channel::oneshot::Receiver<()>,
);
static HOOKS: std::sync::OnceLock<std::sync::Mutex<HashMap<(PathBuf, &'static str), Hook>>> =
    std::sync::OnceLock::new();
pub(super) async fn checkpoint(root: &Path, phase: &'static str) {
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

async fn db(path: &Path) -> Arc<Database> {
    Arc::new(
        Database::init(&format!("sqlite://{}", path.display()))
            .await
            .unwrap(),
    )
}

async fn fixture() -> (tempfile::TempDir, PathBuf, Arc<Database>) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("java");
    let database = db(&dir.path().join("owner.db")).await;
    attach(Some(database.clone()), root.clone()).await.unwrap();
    (dir, root, database)
}

async fn installed(
    root: &Path,
    database: Arc<Database>,
) -> (Arc<ManagedJavaService>, Uuid, PathBuf, String) {
    let source = root.join("fixture.tar.gz");
    tar_fixture(&source);
    let mut package = pin_archive(package(), &source);
    package.id = Uuid::new_v4().to_string();
    let sha256 = package.sha256.clone();
    let service = Arc::new(ManagedJavaService::at(root.into(), database));
    service.initialize().await.unwrap();
    drop(
        super::super::bytes::import(root, source, &package, &Cancellation::new())
            .await
            .unwrap(),
    );
    let id = service.start_package(package).await.unwrap();
    let maintenance = ObjectStore::new(root.join(".managed"))
        .maintenance(&Cancellation::new())
        .await
        .unwrap();
    drop(maintenance);
    assert_eq!(
        service.installation(id).await.unwrap().stage,
        ManagedJavaStage::AwaitingValidation
    );
    let record = super::super::read_record(root, id).unwrap().unwrap();
    (
        service,
        id,
        record.home.unwrap().parent().unwrap().to_owned(),
        sha256,
    )
}

#[async_std::test]
async fn managed_catalogue_precedes_operations_archives_and_homes_and_is_append_only() {
    let (dir, root, first) = fixture().await;
    assert!(!root.join(".managed/operations").exists());
    assert!(!root.join("managed").exists());
    let second = db(&dir.path().join("second.db")).await;
    attach(Some(second.clone()), root.clone()).await.unwrap();
    attach(Some(first), root.clone()).await.unwrap();
    let maintenance = super::super::ownership::maintenance(&root).unwrap();
    let owners = RootCatalog::owners(&maintenance).unwrap();
    assert_eq!(owners.len(), 2);
    drop(maintenance);
    let (_service, _id, _generation, _hash) = installed(&root, second).await;
    assert!(inspect(root).await.is_ok());
}

#[async_std::test]
async fn historical_homes_metadata_and_ephemeral_or_unbound_writers_never_gain_complete_ownership()
{
    for mode in ["home", "metadata", "ephemeral", "unbound"] {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("java");
        let database = db(&dir.path().join("owner.db")).await;
        match mode {
            "home" => fs::create_dir_all(root.join("managed/old/home")).unwrap(),
            "metadata" => {
                fs::create_dir_all(root.join(".managed")).unwrap();
                fs::write(root.join(".managed/old"), b"keep").unwrap();
            }
            "ephemeral" => attach(
                Some(Arc::new(Database::init("sqlite::memory:").await.unwrap())),
                root.clone(),
            )
            .await
            .unwrap(),
            _ => ManagedJavaService::new(root.clone())
                .initialize()
                .await
                .unwrap(),
        }
        attach(Some(database), root.clone()).await.unwrap();
        assert!(inspect(root.clone()).await.is_err(), "{mode}");
        if mode == "metadata" {
            assert_eq!(fs::read(root.join(".managed/old")).unwrap(), b"keep");
        }
    }
}

#[async_std::test]
async fn every_catalogued_sql_registration_and_operation_retains_homes_and_archive_bytes() {
    let (dir, root, first) = fixture().await;
    let second = db(&dir.path().join("second.db")).await;
    attach(Some(second.clone()), root.clone()).await.unwrap();
    let (_service, id, generation, hash) = installed(&root, first.clone()).await;
    let home = root.join(&generation).join("jdk");
    // Synthetic archive fixture extracts jre, not the arbitrary registered path.
    // Missing placements still retain intended generations; selection is irrelevant.
    let new = NewJavaRuntime {
        version: Some("21".into()),
        major: Some(21),
        arch: None,
        exec_path: Some(home.join("bin/java")),
        home_path: Some(home),
        vendor: None,
        source: JavaSource::Managed,
    };
    DbJavaStore::at(second, root.clone())
        .insert(&new)
        .await
        .unwrap();
    let snapshot = super::snapshot(&root).await.unwrap();
    assert!(snapshot.homes.contains(&generation));
    assert!(snapshot.archives.contains(&hash));
    snapshot.close().await;
    fs::remove_file(root.join(".managed/operations").join(format!("{id}.json"))).unwrap();
    let snapshot = super::snapshot(&root).await.unwrap();
    assert!(snapshot.homes.contains(&generation));
    assert!(snapshot.archives.contains(&hash));
    snapshot.close().await;
}

#[async_std::test]
async fn surviving_owned_home_keeps_source_archive_even_after_sql_and_operation_roots_disappear() {
    let (_dir, root, database) = fixture().await;
    let (_service, id, generation, hash) = installed(&root, database).await;
    fs::remove_file(root.join(".managed/operations").join(format!("{id}.json"))).unwrap();
    let snapshot = snapshot(&root).await.unwrap();
    assert!(snapshot.homes.is_empty());
    assert!(snapshot.archives.contains(&hash));
    snapshot.close().await;
    let owner = HomeOwner::read(&root.join(&generation)).unwrap();
    assert_eq!(owner.operation, id);
    assert!(owner.database.is_some());
}

#[async_std::test]
async fn missing_replaced_or_lost_historical_databases_block_java_roots_without_recreating_them() {
    for mode in ["missing", "replaced", "identity"] {
        let (dir, root, database) = fixture().await;
        let path = dir.path().join("owner.db");
        if mode == "identity" {
            sqlx::query("DROP TRIGGER retention_identity_delete")
                .execute(&database.pool)
                .await
                .unwrap();
            sqlx::query("DELETE FROM retention_identity")
                .execute(&database.pool)
                .await
                .unwrap();
        } else {
            database.pool.close().await;
            if mode == "missing" {
                fs::remove_file(&path).unwrap();
            } else {
                fs::rename(&path, dir.path().join("old.db")).unwrap();
                let replacement = db(&path).await;
                replacement.pool.close().await;
            }
        }
        assert!(inspect(root).await.is_err(), "{mode}");
        if mode == "missing" {
            assert!(!path.exists());
        }
    }
}

#[async_std::test]
async fn legacy_unknown_future_and_conflicting_home_markers_defer_the_whole_java_domain() {
    for mode in [
        "missing",
        "corrupt",
        "future",
        "database",
        "operation",
        "digest",
        "home",
        "version",
    ] {
        let (_dir, root, database) = fixture().await;
        let (_service, _id, generation, _hash) = installed(&root, database).await;
        let marker = root
            .join(&generation)
            .join(super::super::ownership::HOME_MARKER);
        if mode == "missing" {
            fs::remove_file(&marker).unwrap();
        } else if mode == "corrupt" {
            fs::write(&marker, b"broken").unwrap();
        } else {
            let mut json: serde_json::Value =
                serde_json::from_slice(&fs::read(&marker).unwrap()).unwrap();
            match mode {
                "future" => json["created"] = u64::MAX.into(),
                "database" => json["database"] = "ab".repeat(16).into(),
                "operation" => json["operation"] = Uuid::new_v4().to_string().into(),
                "digest" => json["sha256"] = "ab".repeat(32).into(),
                "home" => json["home"] = "../outside".into(),
                _ => json["version"] = 99.into(),
            }
            fs::write(&marker, serde_json::to_vec(&json).unwrap()).unwrap();
        }
        let before = fs::read(root.join(&generation).join("jre/bin/java")).unwrap();
        assert!(inspect(root.clone()).await.is_err(), "{mode}");
        assert_eq!(
            fs::read(root.join(generation).join("jre/bin/java")).unwrap(),
            before
        );
    }
}

#[async_std::test]
async fn active_unknown_future_operation_records_and_pending_scratch_are_not_absent_roots() {
    for mode in [
        "active",
        "owner",
        "field",
        "snapshot-field",
        "package-field",
        "future",
        "scratch",
        "import",
    ] {
        let (_dir, root, database) = fixture().await;
        let (_service, id, _generation, _hash) = installed(&root, database).await;
        if ["scratch", "import"].contains(&mode) {
            let scratch = root.join(if mode == "scratch" {
                ".managed/staging/unknown"
            } else {
                ".managed/bytes/staging/unknown"
            });
            fs::create_dir_all(scratch).unwrap();
        } else {
            let path = root.join(".managed/operations").join(format!("{id}.json"));
            let mut json: serde_json::Value =
                serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
            match mode {
                "active" => {
                    json["snapshot"]["stage"] =
                        serde_json::to_value(ManagedJavaStage::Downloading).unwrap()
                }
                "owner" => json["owner"] = serde_json::Value::Null,
                "future" => json["snapshot"]["created_at"] = "2999-01-01T00:00:00Z".into(),
                "snapshot-field" => json["snapshot"]["new-home-root"] = true.into(),
                "package-field" => json["snapshot"]["package"]["new-archive-root"] = true.into(),
                _ => json["unknown-root-role"] = true.into(),
            }
            fs::write(path, serde_json::to_vec(&json).unwrap()).unwrap();
        }
        assert!(inspect(root).await.is_err(), "{mode}");
    }
}

#[async_std::test]
async fn dropped_java_roots_inspector_keeps_all_sql_write_locks_and_admission_until_completion() {
    let (dir, root, database) = fixture().await;
    let (_service, _id, _generation, _hash) = installed(&root, database).await;
    let (entered, receiver) = futures::channel::oneshot::channel();
    let (resume, wait) = futures::channel::oneshot::channel();
    HOOKS
        .get_or_init(Default::default)
        .lock()
        .unwrap()
        .insert((root.clone(), "snapshot"), (entered, wait));
    let mut caller = Box::pin(inspect(root.clone()));
    match futures::future::select(receiver, caller.as_mut()).await {
        futures::future::Either::Left((result, _)) => result.unwrap(),
        futures::future::Either::Right((result, _)) => panic!("finished early: {result:?}"),
    };
    drop(caller);
    assert!(ObjectStore::new(root.join(".managed/bytes"))
        .try_access()
        .is_err());
    assert!(ObjectStore::new(root.join(".managed"))
        .try_access()
        .is_err());
    let options: sqlx::sqlite::SqliteConnectOptions =
        format!("sqlite://{}", dir.path().join("owner.db").display())
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
    let done = ObjectStore::new(root.join(".managed"))
        .access(&Cancellation::new())
        .await
        .unwrap();
    drop(done);
    sqlx::query("UPDATE java_runtimes SET vendor='probe'")
        .execute(&probe)
        .await
        .unwrap();
    probe.close().await;
}

#[cfg(unix)]
#[async_std::test]
async fn canonical_java_root_aliases_share_gate_and_escaping_homes_or_markers_preserve_external_bytes(
) {
    let (dir, root, database) = fixture().await;
    let (_service, _id, generation, _hash) = installed(&root, database).await;
    let alias = dir.path().join("alias");
    std::os::unix::fs::symlink(&root, &alias).unwrap();
    assert!(inspect(alias.clone()).await.is_ok());
    let writer = ObjectStore::new(root.join(".managed"))
        .try_access()
        .unwrap();
    assert!(inspect(alias).await.is_err());
    drop(writer);
    let marker = root
        .join(&generation)
        .join(super::super::ownership::HOME_MARKER);
    let outside = dir.path().join("outside");
    fs::write(&outside, fs::read(&marker).unwrap()).unwrap();
    fs::remove_file(&marker).unwrap();
    std::os::unix::fs::symlink(&outside, &marker).unwrap();
    let before = fs::read(&outside).unwrap();
    assert!(inspect(root).await.is_err());
    assert_eq!(fs::read(outside).unwrap(), before);
}

#[test]
fn published_home_marker_is_no_clobber_and_never_adopts_archive_supplied_metadata() {
    let dir = tempfile::tempdir().unwrap();
    let owner = HomeOwner::new(Uuid::new_v4(), &package(), "jre".into(), None).unwrap();
    let marker = dir.path().join(super::super::ownership::HOME_MARKER);
    fs::write(&marker, b"foreign marker").unwrap();
    assert!(owner.write(dir.path()).is_err());
    assert_eq!(fs::read(marker).unwrap(), b"foreign marker");
}

#[cfg(unix)]
#[async_std::test]
async fn manual_external_alias_into_managed_home_retains_generation_even_with_missing_executable() {
    let (dir, root, database) = fixture().await;
    let (_service, id, generation, hash) = installed(&root, database.clone()).await;
    fs::remove_file(root.join(".managed/operations").join(format!("{id}.json"))).unwrap();
    let alias = dir.path().join("manual-home");
    std::os::unix::fs::symlink(root.join(&generation).join("jre"), &alias).unwrap();
    DbJavaStore::new(database)
        .insert(&NewJavaRuntime {
            version: Some("21".into()),
            major: Some(21),
            arch: None,
            exec_path: Some(alias.join("bin/missing-java")),
            home_path: Some(alias),
            vendor: None,
            source: JavaSource::Manual,
        })
        .await
        .unwrap();
    let snapshot = snapshot(&root).await.unwrap();
    assert!(snapshot.homes.contains(&generation));
    assert!(snapshot.archives.contains(&hash));
    snapshot.close().await;
}

#[async_std::test]
async fn unknown_sql_sources_and_relocated_managed_pairs_block_but_external_manual_java_is_not_adopted(
) {
    for mode in ["manual", "managed", "future"] {
        let (dir, root, database) = fixture().await;
        let external = dir.path().join("external");
        sqlx::query(
            "INSERT INTO java_runtimes(version,exec_path,home_path,source) VALUES('21',?,?,?)",
        )
        .bind(external.join("bin/java").to_str().unwrap())
        .bind(external.to_str().unwrap())
        .bind(mode)
        .execute(&database.pool)
        .await
        .unwrap();
        let result = inspect(root).await;
        if mode == "manual" {
            assert_eq!(result.unwrap(), (0, 0));
        } else {
            assert!(result.is_err(), "{mode}");
        }
    }
}
