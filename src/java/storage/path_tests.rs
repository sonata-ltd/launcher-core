use super::*;
use crate::utils::{cancellation::Cancellation, object_store::ObjectStore};
use std::{fs, path::Path, time::Duration};

type Hook = (
    futures::channel::oneshot::Sender<()>,
    futures::channel::oneshot::Receiver<()>,
);
static HOOKS: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<PathBuf, Hook>>> =
    std::sync::OnceLock::new();
pub(super) async fn checkpoint(root: &Path) {
    let hook = HOOKS
        .get_or_init(Default::default)
        .lock()
        .unwrap()
        .remove(root);
    if let Some((entered, resume)) = hook {
        let _ = entered.send(());
        let _ = resume.await;
    }
    crash_checkpoint(root, "before-commit");
}

pub(super) fn crash_checkpoint(root: &Path, phase: &str) {
    if std::env::var_os("SONATA_TEST_JAVA_PATH_ROOT")
        .is_some_and(|value| Path::new(&value).join("java") == root)
        && std::env::var("SONATA_TEST_JAVA_PATH_PHASE").ok().as_deref() == Some(phase)
    {
        std::process::exit(77);
    }
}

#[async_std::test]
async fn managed_java_path_crash_migrator() {
    let Some(root) = std::env::var_os("SONATA_TEST_JAVA_PATH_ROOT").map(PathBuf::from) else {
        return;
    };
    let db = Arc::new(
        Database::init(&format!("sqlite://{}", root.join("java.db").display()))
            .await
            .unwrap(),
    );
    paths::migrate(db, root.join("java")).await.unwrap();
    panic!("missed Java SQL path crash boundary");
}

#[async_std::test]
async fn process_exit_before_and_after_path_commit_preserves_ids_and_replays_without_guessing() {
    for phase in ["before-commit", "committed"] {
        let (dir, db, root) = fixture().await;
        let runtime = managed(&root);
        bytes(&runtime);
        DbJavaStore::new(db.clone()).insert(&runtime).await.unwrap();
        let before = snapshot(&db).await;
        let directory = dir.path().to_owned();
        let phase_arg = phase.to_owned();
        let output = async_std::task::spawn_blocking(move || {
            std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "java::storage::path_tests::managed_java_path_crash_migrator",
                    "--nocapture",
                ])
                .env("SONATA_TEST_JAVA_PATH_ROOT", directory)
                .env("SONATA_TEST_JAVA_PATH_PHASE", phase_arg)
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
        if phase == "before-commit" {
            assert_eq!(snapshot(&db).await, before);
        } else {
            assert_eq!(
                snapshot(&db).await[0].4,
                "managed/package/generation/jre/bin/java"
            );
        }
        paths::migrate(db.clone(), root.clone()).await.unwrap();
        let after = snapshot(&db).await;
        assert_eq!(after[0].0, before[0].0);
        assert_eq!(paths::migrate(db, root).await.unwrap().converted, 0);
        assert_eq!(fs::read(runtime.exec_path.unwrap()).unwrap(), b"Java bytes");
    }
}

#[cfg(unix)]
#[async_std::test]
async fn non_unicode_managed_input_is_not_lossily_rewritten_or_partially_inserted() {
    use std::os::unix::ffi::OsStringExt;
    let (_dir, db, root) = fixture().await;
    let store = DbJavaStore::at(db.clone(), root.clone());
    let mut invalid = managed(&root);
    invalid.exec_path = Some(
        root.join("managed")
            .join(std::ffi::OsString::from_vec(b"non-unicode-\xff".to_vec())),
    );
    assert!(store.insert(&invalid).await.is_err());
    assert!(store.insert_many(&[managed(&root), invalid]).await.is_err());
    assert!(snapshot(&db).await.is_empty());
}

async fn fixture() -> (tempfile::TempDir, Arc<Database>, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let db = Arc::new(
        Database::init(&format!(
            "sqlite://{}",
            dir.path().join("java.db").display()
        ))
        .await
        .unwrap(),
    );
    let root = dir.path().join("java");
    fs::create_dir(&root).unwrap();
    (dir, db, root)
}

fn managed(root: &Path) -> NewJavaRuntime {
    NewJavaRuntime {
        version: Some("21.0.5+11".into()),
        major: Some(21),
        arch: Some("amd64".into()),
        exec_path: Some(root.join("managed/package/generation/jre/bin/java")),
        home_path: Some(root.join("managed/package/generation/jre")),
        vendor: Some("Test".into()),
        source: JavaSource::Managed,
    }
}

fn bytes(runtime: &NewJavaRuntime) {
    let exec = runtime.exec_path.as_ref().unwrap();
    fs::create_dir_all(exec.parent().unwrap()).unwrap();
    fs::write(exec, b"Java bytes").unwrap();
}

async fn snapshot(
    db: &Database,
) -> Vec<(
    i64,
    String,
    i64,
    Option<String>,
    String,
    String,
    Option<String>,
    String,
)> {
    sqlx::query_as("SELECT id,version,major,arch,exec_path,home_path,vendor,source FROM java_runtimes ORDER BY id").fetch_all(&db.pool).await.unwrap()
}

#[async_std::test]
async fn managed_upgrade_keeps_identity_bytes_metadata_associations_and_relative_paths_survive_relocation(
) {
    let (dir, db, root) = fixture().await;
    let runtime = managed(&root);
    bytes(&runtime);
    let legacy = DbJavaStore::new(db.clone());
    let java = legacy.insert(&runtime).await.unwrap();
    // Use a real instance so FK ownership is exercised, not an orphan fixture.
    sqlx::query("INSERT INTO instances(id,name,dir,version,loader,manifest_url,meta_provider) VALUES(123,'Java owner','owner','1.21','vanilla','unused','Prism')").execute(&db.pool).await.unwrap();
    sqlx::query("INSERT INTO instances_settings(instance_id,java_runtime) VALUES(123,?)")
        .bind(java.id)
        .execute(&db.pool)
        .await
        .unwrap();
    let before = snapshot(&db).await;
    let identity = db.retention_owner().await.unwrap().unwrap().identity;
    let report = paths::migrate(db.clone(), root.clone()).await.unwrap();
    assert_eq!((report.converted, report.unresolved), (1, 0));
    let after = snapshot(&db).await;
    assert_eq!(after[0].4, "managed/package/generation/jre/bin/java");
    assert_eq!(after[0].5, "managed/package/generation/jre");
    let mut normalized = after.clone();
    normalized[0].4 = before[0].4.clone();
    normalized[0].5 = before[0].5.clone();
    assert_eq!(normalized, before);
    let selected: i64 =
        sqlx::query_scalar("SELECT java_runtime FROM instances_settings WHERE instance_id=123")
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(selected, java.id);
    assert_eq!(
        db.retention_owner().await.unwrap().unwrap().identity,
        identity
    );
    assert_eq!(
        paths::migrate(db.clone(), root.clone())
            .await
            .unwrap()
            .converted,
        0
    );
    let moved = dir.path().join("moved-java");
    fs::rename(&root, &moved).unwrap();
    let relocated = DbJavaStore::at(db.clone(), moved.clone());
    let resolved = relocated.get_by_id(java.id).await.unwrap().unwrap();
    assert_eq!(
        resolved.exec_path,
        moved
            .join("managed/package/generation/jre/bin/java")
            .to_str()
            .unwrap()
    );
    assert_eq!(fs::read(resolved.exec_path).unwrap(), b"Java bytes");
    assert!(legacy.get_by_id(java.id).await.is_err());
    assert_eq!(paths::migrate(db, moved).await.unwrap().converted, 0);
}

#[async_std::test]
async fn root_aware_java_writers_store_relative_managed_pairs_but_keep_external_paths_absolute() {
    let (_dir, db, root) = fixture().await;
    let store = DbJavaStore::at(db.clone(), root.clone());
    let runtime = managed(&root);
    bytes(&runtime);
    let java = store.insert(&runtime).await.unwrap();
    assert!(Path::new(&java.exec_path).is_absolute());
    assert_eq!(
        snapshot(&db).await[0].4,
        "managed/package/generation/jre/bin/java"
    );
    assert!(store
        .exists_by_path(runtime.exec_path.as_ref().unwrap())
        .await
        .unwrap());
    assert!(store.insert(&runtime).await.is_err());
    let mut external = managed(&root);
    external.source = JavaSource::Manual;
    external.exec_path = Some(root.join("external/bin/java"));
    external.home_path = Some(root.join("external"));
    store.insert(&external).await.unwrap();
    let rows = snapshot(&db).await;
    assert_eq!(rows[1].4, external.exec_path.unwrap().to_str().unwrap());
}

#[async_std::test]
async fn managed_upgrade_never_guesses_external_or_old_roots_and_keeps_discovered_sources() {
    let (dir, db, root) = fixture().await;
    let legacy = DbJavaStore::new(db.clone());
    for (location, source) in [
        (root.clone(), JavaSource::Discovered),
        (dir.path().join("external"), JavaSource::Managed),
        (dir.path().join("old-java"), JavaSource::Managed),
    ] {
        let mut runtime = managed(&location);
        runtime.source = source;
        legacy.insert(&runtime).await.unwrap();
    }
    let before = snapshot(&db).await;
    let report = paths::migrate(db.clone(), root).await.unwrap();
    assert_eq!((report.converted, report.unresolved), (0, 2));
    assert_eq!(snapshot(&db).await, before);
}

#[async_std::test]
async fn missing_managed_files_upgrade_but_conflicting_sql_identity_is_not_merged_or_reassigned() {
    let (_dir, db, root) = fixture().await;
    let runtime = managed(&root);
    let legacy = DbJavaStore::new(db.clone());
    legacy.insert(&runtime).await.unwrap();
    let report = paths::migrate(db.clone(), root.clone()).await.unwrap();
    assert_eq!(report.converted, 1);
    // A historical duplicate representation is unknown semantic ownership. Keep
    // both IDs and the external/absolute record, never merge FK associations.
    legacy.insert(&runtime).await.unwrap();
    let before = snapshot(&db).await;
    let report = paths::migrate(db.clone(), root).await.unwrap();
    assert_eq!((report.converted, report.unresolved), (0, 1));
    assert_eq!(snapshot(&db).await, before);
}

#[async_std::test]
async fn root_aware_bulk_insert_deduplicates_legacy_absolute_and_relative_managed_executables() {
    let (_dir, db, root) = fixture().await;
    let legacy = DbJavaStore::new(db.clone());
    let runtime = managed(&root);
    legacy.insert(&runtime).await.unwrap();
    let store = DbJavaStore::at(db.clone(), root.clone());
    assert!(store
        .insert_many(&[managed(&root)])
        .await
        .unwrap()
        .is_empty());
    paths::migrate(db.clone(), root.clone()).await.unwrap();
    assert!(store
        .insert_many(&[managed(&root), managed(&root)])
        .await
        .unwrap()
        .is_empty());
    assert_eq!(snapshot(&db).await.len(), 1);
}

#[async_std::test]
async fn unsafe_managed_writes_fail_before_sql_and_unsafe_relative_rows_require_validated_root_aware_reads(
) {
    let (dir, db, root) = fixture().await;
    let store = DbJavaStore::at(db.clone(), root.clone());
    for mode in ["outside", "traversal", "backslash", "wrong-home", "scratch"] {
        let mut runtime = managed(&root);
        runtime.exec_path = Some(match mode {
            "outside" => dir.path().join("outside/bin/java"),
            "traversal" => root.join("managed/../outside/bin/java"),
            "backslash" => "managed\\jre\\bin\\java".into(),
            "wrong-home" => root.join("managed/another/jre/bin/java"),
            _ => root.join(".managed/staging/bin/java"),
        });
        assert!(store.insert(&runtime).await.is_err(), "{mode}");
        assert!(store.insert_many(&[runtime]).await.is_err(), "{mode}");
        assert!(snapshot(&db).await.is_empty());
    }
    sqlx::query("INSERT INTO java_runtimes(version,exec_path,home_path,source) VALUES('21','managed/../outside/bin/java','managed/jre','managed')").execute(&db.pool).await.unwrap();
    assert!(store.list_all().await.is_err());
    assert!(DbJavaStore::new(db.clone()).list_all().await.is_err());
    let before = snapshot(&db).await;
    assert_eq!(
        paths::migrate(db.clone(), root).await.unwrap().unresolved,
        1
    );
    assert_eq!(snapshot(&db).await, before);
}

#[cfg(unix)]
#[async_std::test]
async fn canonical_root_aliases_upgrade_owned_paths_while_escaping_ancestors_preserve_external_bytes(
) {
    let (dir, db, root) = fixture().await;
    let alias = dir.path().join("alias");
    std::os::unix::fs::symlink(&root, &alias).unwrap();
    let runtime = managed(&root);
    bytes(&runtime);
    DbJavaStore::new(db.clone()).insert(&runtime).await.unwrap();
    paths::migrate(db.clone(), alias.clone()).await.unwrap();
    let store = DbJavaStore::at(db.clone(), alias.clone());
    assert_eq!(
        store.list_all().await.unwrap()[0].exec_path,
        runtime.exec_path.as_ref().unwrap().to_str().unwrap()
    );
    assert!(store
        .exists_by_path(&alias.join("managed/package/generation/jre/bin/java"))
        .await
        .unwrap());
    let outside = dir.path().join("outside");
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("victim"), b"keep").unwrap();
    std::os::unix::fs::symlink(&outside, root.join("managed/escape")).unwrap();
    let mut runtime = managed(&root);
    runtime.exec_path = Some(root.join("managed/escape/victim"));
    runtime.home_path = Some(root.join("managed/escape"));
    DbJavaStore::new(db.clone()).insert(&runtime).await.unwrap();
    let before = snapshot(&db).await;
    assert_eq!(
        paths::migrate(db.clone(), alias).await.unwrap().unresolved,
        1
    );
    assert_eq!(snapshot(&db).await, before);
    assert!(store.insert(&runtime).await.is_err());
    assert_eq!(fs::read(outside.join("victim")).unwrap(), b"keep");
}

#[async_std::test]
async fn dropping_managed_path_upgrade_caller_keeps_sql_and_admission_until_commit() {
    let (dir, db, root) = fixture().await;
    DbJavaStore::new(db.clone())
        .insert(&managed(&root))
        .await
        .unwrap();
    let (entered, receiver) = futures::channel::oneshot::channel();
    let (resume, wait) = futures::channel::oneshot::channel();
    HOOKS
        .get_or_init(Default::default)
        .lock()
        .unwrap()
        .insert(root.clone(), (entered, wait));
    let mut caller = Box::pin(paths::migrate(db.clone(), root.clone()));
    match futures::future::select(receiver, caller.as_mut()).await {
        futures::future::Either::Left((result, _)) => result.unwrap(),
        futures::future::Either::Right((result, _)) => panic!("finished early: {result:?}"),
    };
    drop(caller);
    assert!(ObjectStore::new(root.join(".managed"))
        .try_access()
        .is_err());
    let options: sqlx::sqlite::SqliteConnectOptions =
        format!("sqlite://{}", dir.path().join("java.db").display())
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
    assert_eq!(
        snapshot(&db).await[0].4,
        "managed/package/generation/jre/bin/java"
    );
    probe.close().await;
}

#[async_std::test]
async fn managed_writer_admission_defers_path_upgrade_without_sql_changes_and_retry_is_idempotent()
{
    let (_dir, db, root) = fixture().await;
    DbJavaStore::new(db.clone())
        .insert(&managed(&root))
        .await
        .unwrap();
    let before = snapshot(&db).await;
    let store = ObjectStore::new(root.join(".managed"));
    let writer = store.try_access().unwrap();
    assert!(paths::migrate(db.clone(), root.clone()).await.is_err());
    assert_eq!(snapshot(&db).await, before);
    drop(writer);
    assert_eq!(
        paths::migrate(db.clone(), root.clone())
            .await
            .unwrap()
            .converted,
        1
    );
    assert_eq!(paths::migrate(db, root).await.unwrap().converted, 0);
}

#[async_std::test]
async fn production_java_service_upgrade_runs_without_discovery_or_recovery_and_resolves_managed_rows(
) {
    let (_dir, db, root) = fixture().await;
    let legacy = DbJavaStore::new(db.clone());
    let java = legacy.insert(&managed(&root)).await.unwrap();
    let service = crate::java::service::JavaService::new(
        db.clone(),
        crate::bus::EventBus::new(16),
        root.clone(),
    );
    service.initialize().await.unwrap();
    service.initialize().await.unwrap();
    let all = service.list_all().await.unwrap();
    assert_eq!(all[0].id, java.id);
    assert!(Path::new(&all[0].exec_path).is_absolute());
    assert_eq!(
        snapshot(&db).await[0].4,
        "managed/package/generation/jre/bin/java"
    );
    assert!(
        !crate::data::state::get_flag(&db, crate::data::state::JAVA_DISCOVERY_DONE)
            .await
            .unwrap()
    );
    assert!(!root.join(".managed/operations").exists());
}
