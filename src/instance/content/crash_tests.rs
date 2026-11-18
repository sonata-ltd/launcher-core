//! Real process termination: exit skips destructors while SQLite/files survive.
//! This deliberately does not simulate a machine power cut or an OS crash.
use super::*;
use crate::instance::content::{
    catalog::types::CatalogRelease, install::PlannedRelease, journal::Placement,
};

#[derive(serde::Serialize, serde::Deserialize)]
struct Scenario {
    instance: InstanceId,
    operation: uuid::Uuid,
    phase: String,
    release: CatalogRelease,
}

const CHILD_TEST: &str = "instance::content::api_tests::crash_tests::crash_writer";
const CHILD_ROOT: &str = "SONATA_TEST_CONTENT_CRASH_ROOT";

#[test]
fn crash_writer() {
    let Some(root) = std::env::var_os(CHILD_ROOT).map(std::path::PathBuf::from) else {
        return;
    };
    async_std::task::block_on(async {
        let scenario: Scenario =
            serde_json::from_slice(&std::fs::read(root.join("scenario.json")).unwrap()).unwrap();
        let paths = LauncherPaths::resolve(&Config {
            root_override: Some(root.clone()),
            ..Config::init()
        })
        .await
        .unwrap();
        let db = Arc::new(
            Database::init(&format!("sqlite://{}", root.join("crash.db").display()))
                .await
                .unwrap(),
        );
        let dir: String = sqlx::query_scalar("SELECT dir FROM instances WHERE id=?")
            .bind(scenario.instance.get())
            .fetch_one(&db.pool)
            .await
            .unwrap();
        let instance_root = paths.instances().join(dir);
        let store = Arc::new(ObjectStore::new(paths.root().join("content-store")));
        let _store_lock = store.lock_exclusive().unwrap();
        let cancel = Cancellation::default();
        let mut second = scenario.release.clone();
        second.file.file_name = "second.jar".into();
        let releases = [
            PlannedRelease {
                release: scenario.release,
                adopt_existing: false,
            },
            PlannedRelease {
                release: second,
                adopt_existing: false,
            },
        ];
        if matches!(scenario.phase.as_str(), "staged" | "intent") {
            let signature = crate::instance::content::inventory::signature(instance_root.clone())
                .await
                .unwrap();
            crate::instance::content::install::execute(
                db,
                store,
                scenario.instance,
                &instance_root,
                scenario.operation,
                &releases,
                &signature,
                &cancel,
                |done, publishing| {
                    if (scenario.phase == "staged" && done == 2 && !publishing)
                        || (scenario.phase == "intent" && publishing)
                    {
                        std::process::exit(73);
                    }
                },
            )
            .await
            .unwrap();
            panic!("execution missed crash checkpoint");
        }

        storage::begin_preparation(
            &db,
            scenario.operation,
            scenario.instance,
            &store.root_key().unwrap(),
        )
        .await
        .unwrap();
        if scenario.phase == "preparing" {
            std::process::exit(73);
        }
        let object = store
            .find_sha1(releases[0].release.file.sha1.as_deref().unwrap(), &cancel)
            .unwrap()
            .unwrap();
        let object_id = objects::register_leased(&db, &object.digests, true, scenario.operation)
            .await
            .unwrap();
        let files: Vec<_> = releases
            .iter()
            .map(|action| PublishedFile {
                name: action.release.file.file_name.clone(),
                sha256: object.digests.sha256.clone(),
                object_id,
                provider: action.release.key.project.provider,
                project_id: action.release.key.project.project_id.clone(),
                release_id: action.release.key.release_id.clone(),
                placement: Placement::Copy,
                dependencies: Some(vec![]),
            })
            .collect();
        storage::prepare(&db, scenario.operation, scenario.instance, &files)
            .await
            .unwrap();
        for (index, file) in files.iter().enumerate() {
            let staged = instance_root.join(format!("crash-stage-{index}"));
            crate::utils::files::placement::copy_new(&object.path, &staged, &cancel).unwrap();
            crate::utils::files::publish::publish_new(
                staged,
                instance_root.join("mods").join(&file.name),
            )
            .unwrap();
            if scenario.phase == "first-file" {
                std::process::exit(73);
            }
        }
        storage::complete(&db, scenario.operation, scenario.instance, &files)
            .await
            .unwrap();
        if scenario.phase == "completed" {
            std::process::exit(73);
        }
        assert_eq!(scenario.phase, "cleaned");
        storage::clear_completed(&db, Some(scenario.operation))
            .await
            .unwrap();
        std::process::exit(73);
    });
}

async fn terminate_writer(fixture: &Fixture, phase: &str) -> uuid::Uuid {
    let (_, _, action) = cached_release(fixture).await;
    let operation = uuid::Uuid::new_v4();
    std::fs::write(
        fixture.paths.root().join("scenario.json"),
        serde_json::to_vec(&Scenario {
            instance: fixture.id,
            operation,
            phase: phase.into(),
            release: action.release,
        })
        .unwrap(),
    )
    .unwrap();
    let root = fixture.paths.root().to_owned();
    let output = async_std::task::spawn_blocking(move || {
        std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", CHILD_TEST, "--nocapture"])
            .env(CHILD_ROOT, root)
            .output()
            .unwrap()
    })
    .await;
    assert_eq!(
        output.status.code(),
        Some(73),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    operation
}

#[async_std::test]
async fn process_exit_recovers_each_publication_boundary_without_duplicate_links() {
    for phase in [
        "preparing",
        "staged",
        "intent",
        "first-file",
        "completed",
        "cleaned",
    ] {
        let fixture = fixture_with_database(true).await;
        let operation = terminate_writer(&fixture, phase).await;
        let abandoned = matches!(phase, "preparing" | "staged");
        let leases = storage::leased_objects(&fixture.db, operation)
            .await
            .unwrap();
        assert_eq!(
            leases.len(),
            usize::from(matches!(phase, "staged" | "intent" | "first-file")),
            "{phase}"
        );
        let _service = InstanceContentService::new(
            fixture.db.clone(),
            fixture.paths.clone(),
            fixture.instances.clone(),
            None,
            fixture.operations.clone(),
        )
        .await
        .unwrap();
        assert!(
            !storage::has_pending(&fixture.db, fixture.id).await.unwrap(),
            "{phase}"
        );
        assert!(
            storage::leased_objects(&fixture.db, operation)
                .await
                .unwrap()
                .is_empty(),
            "{phase}"
        );
        for name in ["cached.jar", "second.jar"] {
            let file = fixture.root.join("mods").join(name);
            if abandoned {
                assert!(!file.exists(), "{phase}");
            } else {
                assert_eq!(std::fs::read(file).unwrap(), b"cached mod", "{phase}");
            }
        }
        assert!(recovery::recover(
            &fixture.db,
            &fixture.paths,
            &fixture.instances.get_registry(),
            None
        )
        .await
        .unwrap()
        .is_empty());
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM instance_content")
            .fetch_one(&fixture.db.pool)
            .await
            .unwrap();
        assert_eq!(count, if abandoned { 0 } else { 2 }, "{phase}");
    }
}

#[async_std::test]
async fn process_exit_followed_by_a_user_edit_keeps_the_journal_and_user_bytes() {
    let fixture = fixture_with_database(true).await;
    let operation = terminate_writer(&fixture, "first-file").await;
    let target = fixture.root.join("mods/cached.jar");
    std::fs::write(&target, b"user edit after crash").unwrap();
    let _service = InstanceContentService::new(
        fixture.db.clone(),
        fixture.paths.clone(),
        fixture.instances.clone(),
        None,
        fixture.operations.clone(),
    )
    .await
    .unwrap();
    assert_eq!(std::fs::read(target).unwrap(), b"user edit after crash");
    assert!(!fixture.root.join("mods/second.jar").exists());
    assert!(storage::has_pending(&fixture.db, fixture.id).await.unwrap());
    assert_eq!(
        storage::leased_objects(&fixture.db, operation)
            .await
            .unwrap()
            .len(),
        1
    );
}
