use super::*;
use crate::{
    data::{config::Config, layout::LauncherPaths},
    instance::{
        download::libs::{linked_libs_at, LibInfo, LibsData},
        init::{
            model::{InstalledArtifact, InstalledProfile},
            retention, storage,
        },
    },
    java::policy::JavaPolicy,
    modloader::model::LoaderSpec,
    utils::{cancellation::Cancellation, object_store::ObjectStore},
};
use std::fs;

struct Fixture {
    directory: tempfile::TempDir,
    paths: Arc<LauncherPaths>,
    db: Arc<Database>,
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
    let db = Arc::new(Database::init("sqlite::memory:").await.unwrap());
    for id in [1, 2] {
        sqlx::query("INSERT INTO instances(id,name,dir,version,loader,manifest_url,meta_provider) VALUES(?,?,?,'1.21','vanilla','https://example.invalid','prism')")
            .bind(id).bind(id.to_string()).bind(id.to_string()).execute(&db.pool).await.unwrap();
    }
    retention::attach(db.clone(), paths.libraries().clone(), Cancellation::new())
        .await
        .unwrap();
    Fixture {
        directory,
        paths,
        db,
    }
}

async fn placements(f: &Fixture, path: &Path) {
    sqlx::query("INSERT INTO libraries(id,hash,name,path,native,url) VALUES(1,?,'library',?,1,'https://example.invalid')")
        .bind("ab".repeat(20)).bind(path.to_str().unwrap()).execute(&f.db.pool).await.unwrap();
    sqlx::query(
        "INSERT INTO instances_libraries(instance_id,library_id,position) VALUES(1,1,0),(2,1,0)",
    )
    .execute(&f.db.pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO instance_library_placements(instance_id,position,hash,name,path,native,url) SELECT instance_id,position,l.hash,l.name,l.path,l.native,l.url FROM instances_libraries i JOIN libraries l ON i.library_id=l.id").execute(&f.db.pool).await.unwrap();
}

#[async_std::test]
async fn upgrade_preserves_bytes_roles_positions_associations_and_installed_profile() {
    let f = fixture().await;
    let relative = PathBuf::from("instances/1")
        .join(uuid::Uuid::new_v4().to_string())
        .join("maven/library.jar");
    let absolute = f.paths.libraries().join(&relative);
    fs::create_dir_all(absolute.parent().unwrap()).unwrap();
    fs::write(&absolute, b"installed bytes").unwrap();
    placements(&f, &absolute).await;
    let profile = InstalledProfile {
        format_version: 1,
        minecraft: "1.21".into(),
        loader: LoaderSpec::vanilla(),
        manifest: serde_json::json!({}),
        java: JavaPolicy {
            majors: vec![21],
            allow_newer: false,
        },
        libraries_root: relative.parent().unwrap().to_owned(),
        artifacts: vec![InstalledArtifact {
            path: "library.jar".into(),
            sha1: "ab".repeat(20),
            classpath: true,
            native: true,
        }],
        metadata: Vec::new(),
    };
    storage::publish(&f.db, 1.into(), &profile).await.unwrap();
    let before: String = sqlx::query_scalar("SELECT profile_json FROM instance_installations")
        .fetch_one(&f.db.pool)
        .await
        .unwrap();
    let owner_before: String = sqlx::query_scalar("SELECT identity FROM retention_identity")
        .fetch_one(&f.db.pool)
        .await
        .unwrap();
    let report = migrate(f.db.clone(), f.paths.libraries().clone())
        .await
        .unwrap();
    assert_eq!((report.converted, report.unresolved), (3, 0));
    let records: Vec<(i64, i64, String, i64, String)> = sqlx::query_as("SELECT instance_id,position,path,native,hash FROM instance_library_placements ORDER BY instance_id").fetch_all(&f.db.pool).await.unwrap();
    assert_eq!(
        records,
        vec![
            (
                1,
                0,
                relative.to_str().unwrap().to_owned(),
                1,
                "ab".repeat(20)
            ),
            (
                2,
                0,
                relative.to_str().unwrap().to_owned(),
                1,
                "ab".repeat(20)
            )
        ]
    );
    let old: String = sqlx::query_scalar("SELECT path FROM libraries WHERE id=1")
        .fetch_one(&f.db.pool)
        .await
        .unwrap();
    assert_eq!(old, relative.to_str().unwrap());
    let after: String = sqlx::query_scalar("SELECT profile_json FROM instance_installations")
        .fetch_one(&f.db.pool)
        .await
        .unwrap();
    assert_eq!(after, before);
    let owner_after: String = sqlx::query_scalar("SELECT identity FROM retention_identity")
        .fetch_one(&f.db.pool)
        .await
        .unwrap();
    assert_eq!(owner_after, owner_before);
    assert_eq!(fs::read(&absolute).unwrap(), b"installed bytes");
    assert_eq!(
        linked_libs_at(1.into(), f.db.clone(), f.paths.libraries())
            .await
            .unwrap()[0]
            .path(),
        absolute.to_str().unwrap()
    );
    assert_eq!(
        migrate(f.db, f.paths.libraries().clone())
            .await
            .unwrap()
            .converted,
        0
    );
}

#[async_std::test]
async fn relative_legacy_snapshot_follows_library_root_relocation_without_guessing_old_paths() {
    let f = fixture().await;
    let absolute = f.paths.libraries().join("legacy/library.jar");
    fs::create_dir_all(absolute.parent().unwrap()).unwrap();
    fs::write(&absolute, b"bytes").unwrap();
    placements(&f, &absolute).await;
    migrate(f.db.clone(), f.paths.libraries().clone())
        .await
        .unwrap();
    let moved = f.directory.path().join("moved-libraries");
    fs::rename(f.paths.libraries(), &moved).unwrap();
    let libraries = linked_libs_at(2.into(), f.db.clone(), &moved)
        .await
        .unwrap();
    assert_eq!(
        libraries[0].path(),
        moved.join("legacy/library.jar").to_str().unwrap()
    );
    assert_eq!(fs::read(libraries[0].path()).unwrap(), b"bytes");
    assert_eq!(migrate(f.db, moved).await.unwrap().converted, 0);
}

#[async_std::test]
async fn missing_managed_paths_become_relative_but_unknown_external_and_unsafe_paths_stay_untouched(
) {
    for path in [
        PathBuf::from("/external/old-root/libraries/library.jar"),
        PathBuf::from("../outside.jar"),
        PathBuf::from("unsafe\\path.jar"),
    ] {
        let f = fixture().await;
        placements(&f, &path).await;
        let report = migrate(f.db.clone(), f.paths.libraries().clone())
            .await
            .unwrap();
        assert_eq!((report.converted, report.unresolved), (0, 3));
        let stored: String = sqlx::query_scalar("SELECT path FROM libraries")
            .fetch_one(&f.db.pool)
            .await
            .unwrap();
        assert_eq!(stored, path.to_str().unwrap());
    }
    let f = fixture().await;
    placements(&f, &f.paths.libraries().join("missing/library.jar")).await;
    assert_eq!(
        migrate(f.db.clone(), f.paths.libraries().clone())
            .await
            .unwrap()
            .converted,
        3
    );
    assert_eq!(
        linked_libs_at(1.into(), f.db, f.paths.libraries())
            .await
            .unwrap()[0]
            .path(),
        f.paths
            .libraries()
            .join("missing/library.jar")
            .to_str()
            .unwrap()
    );
}

#[cfg(unix)]
#[async_std::test]
async fn canonical_root_alias_is_migrated_but_escaping_ancestors_are_not_reclassified() {
    let f = fixture().await;
    let alias = f.directory.path().join("library-alias");
    std::os::unix::fs::symlink(f.paths.libraries(), &alias).unwrap();
    placements(&f, &alias.join("missing/library.jar")).await;
    assert_eq!(migrate(f.db.clone(), alias).await.unwrap().converted, 3);
    let outside = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink(outside.path(), f.paths.libraries().join("linked")).unwrap();
    let escaped = f.paths.libraries().join("linked/library.jar");
    sqlx::query("UPDATE libraries SET path=?")
        .bind(escaped.to_str().unwrap())
        .execute(&f.db.pool)
        .await
        .unwrap();
    let report = migrate(f.db.clone(), f.paths.libraries().clone())
        .await
        .unwrap();
    assert_eq!((report.converted, report.unresolved), (0, 1));
    let stored: String = sqlx::query_scalar("SELECT path FROM libraries")
        .fetch_one(&f.db.pool)
        .await
        .unwrap();
    assert_eq!(stored, escaped.to_str().unwrap());
}

#[async_std::test]
async fn root_aware_writer_stores_relative_paths_and_rejects_outside_before_transport() {
    let f = fixture().await;
    let file = f.paths.libraries().join("legacy/library.jar");
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::write(&file, b"library").unwrap();
    let library = LibInfo::new(
        "library".into(),
        files::sha1_file(&file).unwrap(),
        "legacy/library.jar".into(),
        "not-a-network-url".into(),
        true,
    )
    .unwrap();
    let operations =
        crate::data::registry::operation::OperationRegistry::new(crate::bus::EventBus::new(128));
    let op = operations.begin("test".into(), Vec::new());
    let result = LibsData::sync_prepared_at(
        1.into(),
        vec![library],
        f.db.clone(),
        &op,
        f.paths.libraries(),
    )
    .await
    .unwrap();
    assert_eq!(result.classpath, vec![file.clone()]);
    assert_eq!(result.natives, vec![file]);
    let stored: String = sqlx::query_scalar("SELECT path FROM instance_library_placements")
        .fetch_one(&f.db.pool)
        .await
        .unwrap();
    assert_eq!(stored, "legacy/library.jar");
    let outside = LibInfo::new(
        "external".into(),
        "ab".repeat(20),
        f.directory.path().join("outside.jar"),
        "invalid-url".into(),
        false,
    )
    .unwrap();
    assert!(LibsData::sync_prepared_at(
        1.into(),
        vec![outside],
        f.db.clone(),
        &op,
        f.paths.libraries()
    )
    .await
    .is_err());
    assert!(!f.directory.path().join("outside.jar").exists());
    assert_eq!(
        linked_libs_at(1.into(), f.db, f.paths.libraries())
            .await
            .unwrap()
            .len(),
        1
    );
    op.finish(crate::data::registry::operation::message::status::Outcome::Completed);
}

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
}

#[async_std::test]
async fn dropped_migration_caller_keeps_admission_until_sql_commit_completes() {
    let f = fixture().await;
    placements(&f, &f.paths.libraries().join("missing/library.jar")).await;
    let (entered, receiver) = futures::channel::oneshot::channel();
    let (resume, wait) = futures::channel::oneshot::channel();
    HOOKS
        .get_or_init(Default::default)
        .lock()
        .unwrap()
        .insert(f.paths.libraries().clone(), (entered, wait));
    let mut future = Box::pin(migrate(f.db.clone(), f.paths.libraries().clone()));
    match futures::future::select(receiver, future.as_mut()).await {
        futures::future::Either::Left((result, _)) => result.unwrap(),
        futures::future::Either::Right((result, _)) => {
            panic!("migration completed early: {result:?}")
        }
    }
    drop(future);
    let store = ObjectStore::new(f.paths.libraries().clone());
    assert!(store.try_access().is_err());
    resume.send(()).unwrap();
    store.access(&Cancellation::new()).await.unwrap();
    let stored: String = sqlx::query_scalar("SELECT path FROM libraries")
        .fetch_one(&f.db.pool)
        .await
        .unwrap();
    assert_eq!(stored, "missing/library.jar");
}

#[async_std::test]
async fn root_aware_reader_rejects_traversal_and_invalid_checksums() {
    for (path, hash) in [
        ("../outside", "ab".repeat(20)),
        ("unsafe\\path.jar", "ab".repeat(20)),
        ("missing.jar", "zz".repeat(20)),
    ] {
        let f = fixture().await;
        sqlx::query("INSERT INTO instance_library_placements(instance_id,position,hash,name,path,native,url) VALUES(1,0,?,'library',?,0,'unused')")
            .bind(hash).bind(path).execute(&f.db.pool).await.unwrap();
        assert!(
            linked_libs_at(1.into(), f.db, f.paths.libraries())
                .await
                .is_err(),
            "{path}"
        );
    }
}
