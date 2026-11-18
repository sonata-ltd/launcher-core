//! Historical ownership is proven by a recorded placement or a unique physical
//! CAS alias, never equal bytes. Real local FS/SQLite; no GUI, HTTP or JVM.
use super::*;
use crate::instance::content::{install::checkpoints::Pause, store::legacy};
use crate::utils::files::{self, digest::FileStamp};
use futures::FutureExt;
use std::{fs, io, path::Path, time::Duration};

#[derive(Debug, PartialEq, sqlx::FromRow)]
struct Binding {
    id: i64,
    instance_id: i64,
    relative_path: String,
    path_key: Vec<u8>,
    kind: String,
    mod_enabled: Option<bool>,
    placement: String,
    presence: String,
    object_id: Option<i64>,
    preferred_provider_file_id: Option<i64>,
    pinned: bool,
    observed_size: Option<i64>,
    observed_modified: Option<String>,
    observed_identity: Option<Vec<u8>>,
    indexed_fingerprint: Option<Vec<u8>>,
    seen_generation: i64,
}

async fn bindings(f: &Fixture) -> Vec<Binding> {
    sqlx::query_as("SELECT * FROM instance_content ORDER BY id")
        .fetch_all(&f.db.pool)
        .await
        .unwrap()
}

async fn registered_link(f: &Fixture) -> (ObjectStore, std::path::PathBuf, std::path::PathBuf) {
    let (operation, files) = prepared(f).await;
    let store = ObjectStore::new(f.paths.root().join("content-store"));
    let object = store.path_for(&files[0].sha256).unwrap();
    let target = f.root.join("mods/mod.jar");
    fs::hard_link(&object, &target).unwrap();
    storage::complete(&f.db, operation, f.id, &files)
        .await
        .unwrap();
    (store, object, target)
}

async fn startup(f: &Fixture) -> InstanceContentService {
    InstanceContentService::new(
        f.db.clone(),
        f.paths.clone(),
        f.instances.clone(),
        None,
        f.operations.clone(),
    )
    .await
    .unwrap()
}

fn stamp(path: &Path) -> FileStamp {
    FileStamp::from_metadata(&fs::symlink_metadata(path).unwrap())
}

async fn add_binding(f: &Fixture, relative: &Path, same_object: bool) {
    sqlx::query(
        "INSERT INTO instance_content(instance_id,object_id,kind,relative_path,path_key,mod_enabled,placement,preferred_provider_file_id)
         SELECT instance_id,CASE WHEN ? THEN object_id ELSE NULL END,kind,?,?,mod_enabled,?,
                CASE WHEN ? THEN preferred_provider_file_id ELSE NULL END
         FROM instance_content ORDER BY id LIMIT 1",
    )
    .bind(same_object).bind(relative.to_str().unwrap()).bind(files::path_key(relative))
    .bind(if same_object { "hardlink" } else { "external" }).bind(same_object)
    .execute(&f.db.pool).await.unwrap();
}

#[cfg(unix)]
#[async_std::test]
async fn unique_renamed_and_disabled_links_keep_binding_identity_and_provenance() {
    for (name, enabled) in [("renamed.jar", true), ("renamed.jar.disabled", false)] {
        let f = fixture().await;
        let (_, object, target) = registered_link(&f).await;
        sqlx::query(
            "UPDATE instance_content SET pinned=1, presence='missing', observed_size=7,
             observed_modified='old', observed_identity=x'01', indexed_fingerprint=x'02', seen_generation=9",
        ).execute(&f.db.pool).await.unwrap();
        sqlx::query("UPDATE content_provider_files SET dependencies_json='[]'")
            .execute(&f.db.pool)
            .await
            .unwrap();
        let mut expected = bindings(&f).await;
        let provenance: Vec<(i64, i64, String, String, String, Option<String>)> = sqlx::query_as(
            "SELECT id,object_id,project_id,release_id,match_evidence,dependencies_json FROM content_provider_files",
        ).fetch_all(&f.db.pool).await.unwrap();
        let relative = Path::new("mods").join(name);
        let renamed = f.root.join(&relative);
        fs::rename(target, &renamed).unwrap();
        assert_eq!(stamp(&renamed).inode, stamp(&object).inode);
        let permissions = fs::metadata(&renamed).unwrap().permissions();

        let service = startup(&f).await;
        expected[0].relative_path = relative.to_str().unwrap().into();
        expected[0].path_key = files::path_key(&relative);
        expected[0].mod_enabled = Some(enabled);
        expected[0].placement = "copy".into();
        expected[0].presence = "present".into();
        expected[0].observed_size = None;
        expected[0].observed_modified = None;
        expected[0].observed_identity = None;
        expected[0].indexed_fingerprint = None;
        assert_eq!(bindings(&f).await, expected);
        let after: Vec<(i64, i64, String, String, String, Option<String>)> = sqlx::query_as(
            "SELECT id,object_id,project_id,release_id,match_evidence,dependencies_json FROM content_provider_files",
        ).fetch_all(&f.db.pool).await.unwrap();
        assert_eq!(after, provenance);
        assert_eq!(fs::read(&renamed).unwrap(), b"verified bytes");
        assert_eq!(fs::metadata(&renamed).unwrap().permissions(), permissions);
        assert_ne!(stamp(&renamed).inode, stamp(&object).inode);
        fs::write(&renamed, b"local edit").unwrap();
        assert_eq!(fs::read(object).unwrap(), b"verified bytes");
        drop(service);
        f.operations.tasks.shutdown().await;
    }
}

#[cfg(unix)]
#[async_std::test]
async fn renamed_archives_are_found_only_in_their_registered_content_scope() {
    for (kind, directory) in [
        ("resource_pack", "resourcepacks"),
        ("shader_pack", "shaderpacks"),
    ] {
        let f = fixture().await;
        let (_, object, target) = registered_link(&f).await;
        fs::create_dir(f.root.join(directory)).unwrap();
        let recorded = Path::new(directory).join("old.zip");
        sqlx::query(
            "UPDATE instance_content SET kind=?,relative_path=?,path_key=?,mod_enabled=NULL",
        )
        .bind(kind)
        .bind(recorded.to_str().unwrap())
        .bind(files::path_key(&recorded))
        .execute(&f.db.pool)
        .await
        .unwrap();
        let renamed = f.root.join(directory).join("renamed.zip");
        fs::rename(target, &renamed).unwrap();
        let out_of_scope = f.root.join("mods/other.jar");
        fs::hard_link(&object, &out_of_scope).unwrap();
        let outside_stamp = stamp(&out_of_scope);
        let service = startup(&f).await;
        let row = bindings(&f).await.remove(0);
        assert_eq!(
            Path::new(&row.relative_path),
            Path::new(directory).join("renamed.zip")
        );
        assert_eq!(row.kind, kind);
        assert_eq!(row.mod_enabled, None);
        assert_eq!(row.placement, "copy");
        assert_ne!(stamp(&renamed).inode, stamp(&object).inode);
        let outside_after = stamp(&out_of_scope);
        assert_eq!(outside_after.device, outside_stamp.device);
        assert_eq!(outside_after.inode, outside_stamp.inode);
        assert_eq!(outside_after.modified, outside_stamp.modified);
        assert_eq!(fs::read(&out_of_scope).unwrap(), b"verified bytes");
        // Removing another alias changes the shared inode's ctime/link count.
        drop(service);
        f.operations.tasks.shutdown().await;
    }
}

#[async_std::test]
async fn equal_bytes_size_and_mtime_do_not_prove_renamed_hardlink_ownership() {
    let f = fixture().await;
    let (_, object, target) = registered_link(&f).await;
    fs::remove_file(target).unwrap();
    let copy = f.root.join("mods/equal.jar");
    fs::copy(&object, &copy).unwrap();
    fs::File::options()
        .write(true)
        .open(&copy)
        .unwrap()
        .set_modified(fs::metadata(&object).unwrap().modified().unwrap())
        .unwrap();
    assert_eq!(stamp(&copy).size_bytes, stamp(&object).size_bytes);
    assert_eq!(stamp(&copy).modified, stamp(&object).modified);
    assert_eq!(
        files::sha256_file(&copy).unwrap(),
        files::sha256_file(&object).unwrap()
    );
    let before = stamp(&copy);
    let rows = bindings(&f).await;
    let service = startup(&f).await;
    assert_eq!(bindings(&f).await, rows);
    assert_eq!(stamp(&copy), before);
    assert_eq!(fs::read(&copy).unwrap(), b"verified bytes");
    drop(service);
    f.operations.tasks.shutdown().await;
}

#[async_std::test]
async fn ambiguous_inode_aliases_are_not_detached_or_rebound() {
    let f = fixture().await;
    let (_, object, target) = registered_link(&f).await;
    let first = f.root.join("mods/first.jar");
    let second = f.root.join("mods/second.jar.disabled");
    fs::rename(target, &first).unwrap();
    fs::hard_link(&object, &second).unwrap();
    let rows = bindings(&f).await;
    let before = (stamp(&first), stamp(&second));
    let service = startup(&f).await;
    assert_eq!(bindings(&f).await, rows);
    assert_eq!((stamp(&first), stamp(&second)), before);
    assert_eq!(fs::read(object).unwrap(), b"verified bytes");
    drop(service);
    f.operations.tasks.shutdown().await;
}

#[async_std::test]
async fn multiple_historical_bindings_cannot_claim_one_remaining_alias() {
    let f = fixture().await;
    let (_, _, target) = registered_link(&f).await;
    fs::rename(target, f.root.join("mods/remaining.jar")).unwrap();
    add_binding(&f, Path::new("mods/also-missing.jar"), true).await;
    let rows = bindings(&f).await;
    let before = stamp(&f.root.join("mods/remaining.jar"));
    let service = startup(&f).await;
    assert_eq!(bindings(&f).await, rows);
    assert_eq!(stamp(&f.root.join("mods/remaining.jar")), before);
    drop(service);
    f.operations.tasks.shutdown().await;
}

#[async_std::test]
async fn an_occupied_renamed_path_keeps_both_bindings_and_the_alias_unchanged() {
    let f = fixture().await;
    let (_, _, target) = registered_link(&f).await;
    let relative = Path::new("mods/occupied.jar");
    let renamed = f.root.join(relative);
    fs::rename(target, &renamed).unwrap();
    add_binding(&f, relative, false).await;
    let rows = bindings(&f).await;
    let before = stamp(&renamed);
    let service = startup(&f).await;
    assert_eq!(bindings(&f).await, rows);
    assert_eq!(stamp(&renamed), before);
    drop(service);
    f.operations.tasks.shutdown().await;
}

#[async_std::test]
async fn missing_cas_does_not_turn_equal_digest_lookup_into_ownership() {
    let f = fixture().await;
    let (_, object, target) = registered_link(&f).await;
    let renamed = f.root.join("mods/renamed.jar");
    fs::rename(target, &renamed).unwrap();
    fs::remove_file(&object).unwrap();
    let before = stamp(&renamed);
    let rows = bindings(&f).await;
    let service = startup(&f).await;
    assert_eq!(bindings(&f).await, rows);
    assert_eq!(stamp(&renamed), before);
    assert_eq!(fs::read(renamed).unwrap(), b"verified bytes");
    assert!(!object.exists());
    drop(service);
    f.operations.tasks.shutdown().await;
}

#[async_std::test]
async fn a_replaced_cas_file_with_equal_bytes_does_not_identify_the_old_alias() {
    let f = fixture().await;
    let (_, object, target) = registered_link(&f).await;
    let renamed = f.root.join("mods/renamed.jar");
    fs::rename(target, &renamed).unwrap();
    let replacement = object.with_extension("replacement");
    fs::write(&replacement, b"verified bytes").unwrap();
    fs::rename(replacement, &object).unwrap();
    let rows = bindings(&f).await;
    let before = stamp(&renamed);
    let service = startup(&f).await;
    assert_eq!(bindings(&f).await, rows);
    assert_eq!(stamp(&renamed), before);
    assert_eq!(fs::read(&object).unwrap(), b"verified bytes");
    assert_eq!(fs::read(&renamed).unwrap(), b"verified bytes");
    drop(service);
    f.operations.tasks.shutdown().await;
}

#[async_std::test]
async fn aliases_outside_the_known_archive_inventory_are_left_untouched() {
    for name in [
        "renamed.jar",
        "mods/unrecognized.bin",
        "mods/subdir/renamed.jar",
    ] {
        let f = fixture().await;
        let (_, _, target) = registered_link(&f).await;
        let renamed = f.root.join(name);
        fs::create_dir_all(renamed.parent().unwrap()).unwrap();
        fs::rename(target, &renamed).unwrap();
        let rows = bindings(&f).await;
        let before = stamp(&renamed);
        let service = startup(&f).await;
        assert_eq!(bindings(&f).await, rows);
        assert_eq!(stamp(&renamed), before);
        drop(service);
        f.operations.tasks.shutdown().await;
    }
}

#[async_std::test]
async fn a_recorded_path_can_be_isolated_even_when_cas_is_missing() {
    let f = fixture().await;
    let (_, object, target) = registered_link(&f).await;
    fs::remove_file(&object).unwrap();
    let service = startup(&f).await;
    assert_eq!(bindings(&f).await[0].placement, "copy");
    assert_eq!(fs::read(target).unwrap(), b"verified bytes");
    assert!(!object.exists());
    drop(service);
    f.operations.tasks.shutdown().await;
}

#[cfg(unix)]
#[async_std::test]
async fn a_corrupt_renamed_cas_alias_preserves_current_bytes_and_drops_stale_identity() {
    let f = fixture().await;
    let (_, object, target) = registered_link(&f).await;
    let relative = Path::new("mods/changed.jar.disabled");
    let renamed = f.root.join(relative);
    fs::rename(target, &renamed).unwrap();
    fs::write(&renamed, b"user modified shared bytes").unwrap();
    let original = bindings(&f).await.remove(0);
    let service = startup(&f).await;
    let row = bindings(&f).await.remove(0);
    assert_eq!(row.id, original.id);
    assert_eq!(row.instance_id, original.instance_id);
    assert_eq!(Path::new(&row.relative_path), relative);
    assert_eq!(row.mod_enabled, Some(false));
    assert_eq!(row.placement, "external");
    assert_eq!(row.object_id, None);
    assert_eq!(row.preferred_provider_file_id, None);
    assert_eq!(fs::read(&renamed).unwrap(), b"user modified shared bytes");
    assert_eq!(fs::read(&object).unwrap(), b"user modified shared bytes");
    assert_ne!(stamp(&renamed).inode, stamp(&object).inode);
    fs::write(renamed, b"later edit").unwrap();
    assert_eq!(fs::read(object).unwrap(), b"user modified shared bytes");
    drop(service);
    f.operations.tasks.shutdown().await;
}

#[cfg(not(unix))]
#[async_std::test]
async fn unsupported_physical_identity_defers_even_a_unique_renamed_link() {
    let f = fixture().await;
    let (_, _, target) = registered_link(&f).await;
    let renamed = f.root.join("mods/renamed.jar");
    fs::rename(target, &renamed).unwrap();
    let rows = bindings(&f).await;
    let before = stamp(&renamed);
    let service = startup(&f).await;
    assert_eq!(bindings(&f).await, rows);
    assert_eq!(stamp(&renamed), before);
    drop(service);
    f.operations.tasks.shutdown().await;
}

#[cfg(unix)]
#[async_std::test]
async fn a_non_unicode_alias_is_not_lossily_rebound() {
    use std::os::unix::ffi::OsStringExt;
    let f = fixture().await;
    let (_, _, target) = registered_link(&f).await;
    let renamed = f
        .root
        .join("mods")
        .join(std::ffi::OsString::from_vec(b"name-\xff.jar".to_vec()));
    fs::rename(target, &renamed).unwrap();
    let rows = bindings(&f).await;
    let before = stamp(&renamed);
    let service = startup(&f).await;
    assert_eq!(bindings(&f).await, rows);
    assert_eq!(stamp(&renamed), before);
    drop(service);
    f.operations.tasks.shutdown().await;
}

#[cfg(unix)]
#[async_std::test]
async fn symbolic_links_and_symlinked_inventory_directories_are_not_ownership() {
    for linked_directory in [false, true] {
        let f = fixture().await;
        let (_, object, target) = registered_link(&f).await;
        fs::remove_file(target).unwrap();
        let outside = f._directory.path().join("outside");
        fs::create_dir(&outside).unwrap();
        let candidate = if linked_directory {
            fs::hard_link(&object, outside.join("renamed.jar")).unwrap();
            fs::remove_dir(f.root.join("mods")).unwrap();
            std::os::unix::fs::symlink(&outside, f.root.join("mods")).unwrap();
            outside.join("renamed.jar")
        } else {
            std::os::unix::fs::symlink(&object, f.root.join("mods/renamed.jar")).unwrap();
            f.root.join("mods/renamed.jar")
        };
        let rows = bindings(&f).await;
        let before = stamp(&candidate);
        let service = startup(&f).await;
        assert_eq!(bindings(&f).await, rows);
        assert_eq!(stamp(&candidate), before);
        assert_eq!(fs::read(&object).unwrap(), b"verified bytes");
        drop(service);
        f.operations.tasks.shutdown().await;
    }
}

fn assert_lifetime_locks(f: &Fixture, store: &ObjectStore, busy: bool) {
    for path in [
        f.paths.root().join(".launcher-content.lock"),
        f.paths.instances().join(".launcher-content.lock"),
    ] {
        let file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .unwrap();
        let result = crate::utils::object_store::StoreLock::exclusive(file);
        if busy {
            assert_eq!(result.err().unwrap().kind(), io::ErrorKind::WouldBlock);
        } else {
            assert!(result.is_ok());
        }
    }
    let result = store.lock_exclusive();
    if busy {
        assert_eq!(result.err().unwrap().kind(), io::ErrorKind::WouldBlock);
    } else {
        assert!(result.is_ok());
    }
}

#[async_std::test]
async fn dropped_bootstrap_keeps_migration_guards_until_sql_completion_and_shutdown() {
    let f = fixture_with_database(true).await;
    let (store, object, target) = registered_link(&f).await;
    let dir = f.root.file_name().unwrap().to_str().unwrap();
    let mut pause = Pause::new(legacy::checkpoint(f.paths.instances(), dir), "legacy-sql");
    let mut caller = Box::pin(InstanceContentService::new(
        f.db.clone(),
        f.paths.clone(),
        f.instances.clone(),
        None,
        f.operations.clone(),
    ));
    futures::select! {
        _ = pause.entered().fuse() => {},
        result = caller.as_mut().fuse() => panic!("bootstrap completed before migration checkpoint: {}", result.is_ok()),
    }
    // Pause is after isolation, before SQL. A real SQLite writer blocks the
    // completion while the bootstrap future (and its original locks) is dropped.
    let lock = f.db.pool.begin_with("BEGIN IMMEDIATE").await.unwrap();
    drop(caller);
    let activity = f.instances.activity();
    assert!(activity.reserve_content_change(f.id).is_err());
    assert!(activity.reserve_install(f.id).is_err());
    assert!(activity.reserve_launch(f.id).is_err());
    assert_lifetime_locks(&f, &store, true);
    assert_eq!(
        store.try_maintenance().err().unwrap().kind(),
        io::ErrorKind::WouldBlock
    );
    assert_eq!(bindings(&f).await[0].placement, "hardlink");
    assert_eq!(fs::read(&target).unwrap(), b"verified bytes");
    #[cfg(unix)]
    assert_ne!(stamp(&target).inode, stamp(&object).inode);
    assert_eq!(fs::read(&object).unwrap(), b"verified bytes");
    pause.resume();
    let mut shutdown = Box::pin(f.operations.tasks.shutdown());
    assert!(futures::poll!(shutdown.as_mut()).is_pending());
    assert!(activity.reserve_content_change(f.id).is_err());
    assert_lifetime_locks(&f, &store, true);
    lock.rollback().await.unwrap();
    let results = async_std::future::timeout(Duration::from_secs(10), shutdown)
        .await
        .unwrap();
    assert!(results
        .iter()
        .any(|r| r.name == "legacy hardlink migration" && r.error.is_none()));
    assert_eq!(bindings(&f).await[0].placement, "copy");
    assert!(activity.reserve_content_change(f.id).is_ok());
    assert_lifetime_locks(&f, &store, false);
    assert!(store.try_maintenance().is_ok());
    fs::write(&target, b"isolated local edit").unwrap();
    assert_eq!(fs::read(target).unwrap(), b"isolated local edit");
    assert_eq!(fs::read(object).unwrap(), b"verified bytes");
}

#[cfg(unix)]
#[async_std::test]
async fn a_collision_after_copy_cannot_merge_or_replace_bindings() {
    let f = fixture().await;
    let (_, object, target) = registered_link(&f).await;
    let relative = Path::new("mods/renamed.jar");
    let renamed = f.root.join(relative);
    fs::rename(target, &renamed).unwrap();
    let dir = f.root.file_name().unwrap().to_str().unwrap();
    let mut pause = Pause::new(legacy::checkpoint(f.paths.instances(), dir), "legacy-sql");
    let caller = startup(&f);
    let inspect = async {
        pause.entered().await;
        add_binding(&f, relative, false).await;
        let expected = bindings(&f).await;
        pause.resume();
        expected
    };
    let (service, expected) = futures::join!(caller, inspect);
    assert_eq!(bindings(&f).await, expected);
    assert_eq!(fs::read(&renamed).unwrap(), b"verified bytes");
    assert_ne!(stamp(&renamed).inode, stamp(&object).inode);
    drop(service);
    f.operations.tasks.shutdown().await;
}

#[cfg(unix)]
#[async_std::test]
async fn a_replaced_target_after_identity_lookup_is_not_overwritten() {
    let f = fixture().await;
    let (_, object, target) = registered_link(&f).await;
    let renamed = f.root.join("mods/renamed.jar");
    fs::rename(target, &renamed).unwrap();
    let rows = bindings(&f).await;
    let dir = f.root.file_name().unwrap().to_str().unwrap();
    let mut pause = Pause::new(
        legacy::checkpoint(f.paths.instances(), dir),
        "legacy-identified",
    );
    let caller = startup(&f);
    let inspect = async {
        pause.entered().await;
        let replacement = f.root.join("replacement");
        fs::write(&replacement, b"unowned replacement").unwrap();
        fs::rename(replacement, &renamed).unwrap();
        let expected = stamp(&renamed);
        pause.resume();
        expected
    };
    let (service, expected) = futures::join!(caller, inspect);
    assert_eq!(bindings(&f).await, rows);
    assert_eq!(stamp(&renamed), expected);
    assert_eq!(fs::read(renamed).unwrap(), b"unowned replacement");
    assert_eq!(fs::read(object).unwrap(), b"verified bytes");
    drop(service);
    f.operations.tasks.shutdown().await;
}

#[cfg(unix)]
#[async_std::test]
async fn changed_binding_or_new_competing_claim_after_copy_is_not_reassigned() {
    for mutation in ["path_key", "kind", "claim"] {
        let f = fixture().await;
        let (_, object, target) = registered_link(&f).await;
        let renamed = f.root.join("mods/renamed.jar");
        fs::rename(target, &renamed).unwrap();
        let dir = f.root.file_name().unwrap().to_str().unwrap();
        let mut pause = Pause::new(legacy::checkpoint(f.paths.instances(), dir), "legacy-sql");
        let caller = startup(&f);
        let inspect = async {
            pause.entered().await;
            match mutation {
                "path_key" => {
                    sqlx::query("UPDATE instance_content SET path_key=?")
                        .bind(files::path_key(Path::new("mods/other.jar")))
                        .execute(&f.db.pool)
                        .await
                        .unwrap();
                }
                "kind" => {
                    sqlx::query(
                        "UPDATE instance_content SET kind='resource_pack',mod_enabled=NULL",
                    )
                    .execute(&f.db.pool)
                    .await
                    .unwrap();
                }
                "claim" => add_binding(&f, Path::new("mods/also-missing.jar"), true).await,
                _ => unreachable!(),
            }
            let expected = bindings(&f).await;
            pause.resume();
            expected
        };
        let (service, expected) = futures::join!(caller, inspect);
        assert_eq!(bindings(&f).await, expected);
        assert_eq!(fs::read(&renamed).unwrap(), b"verified bytes");
        assert_ne!(stamp(&renamed).inode, stamp(&object).inode);
        drop(service);
        f.operations.tasks.shutdown().await;
    }
}

#[cfg(unix)]
#[async_std::test]
async fn sql_failure_after_isolation_preserves_bytes_cas_and_original_binding() {
    let f = fixture().await;
    let (store, object, target) = registered_link(&f).await;
    let renamed = f.root.join("mods/renamed.jar");
    fs::rename(target, &renamed).unwrap();
    let rows = bindings(&f).await;
    sqlx::query(
        "CREATE TRIGGER reject_legacy_update BEFORE UPDATE ON instance_content
         WHEN NEW.placement='copy' BEGIN SELECT RAISE(ABORT,'test migration SQL failure'); END",
    )
    .execute(&f.db.pool)
    .await
    .unwrap();
    let service = startup(&f).await;
    assert_eq!(bindings(&f).await, rows);
    assert_eq!(fs::read(&renamed).unwrap(), b"verified bytes");
    assert_ne!(stamp(&renamed).inode, stamp(&object).inode);
    fs::write(renamed, b"independent after failure").unwrap();
    assert_eq!(fs::read(object).unwrap(), b"verified bytes");
    drop(service);
    f.operations.tasks.shutdown().await;
    assert_lifetime_locks(&f, &store, false);
    assert!(store.try_maintenance().is_ok());
}
