use crate::{
    instance::{
        download::{artifacts, transfer},
        init::{model::InstalledProfile, storage},
    },
    utils::files,
};

use super::{
    catalog::ModloaderCatalog, components, jobs::InstallJobs, model::*, repository::Repository,
};
use serde_json::json;
use std::{collections::HashMap, path::PathBuf};

#[test]
fn install_job_reservation_and_cancel_are_core_owned() {
    let jobs = InstallJobs::default();
    let job = jobs.begin(1.into()).unwrap();
    assert!(jobs.begin(2.into()).is_err());
    jobs.cancel(1.into());
    assert!(job.cancel.check().is_err());
    drop(job);
    assert!(jobs.begin(2.into()).is_ok());
}

#[test]
fn artifacts_separate_processor_files_and_natives_from_classpath() {
    let mut manifest = json!({"libraries":[{"name":"g:a:1","downloads":{"artifact":{"url":"https://example.invalid/a","sha1":"0000000000000000000000000000000000000000"}}}]});
    let files = vec![
        json!({"name":"g:tool:1","downloads":{"artifact":{"url":"https://example.invalid/tool"}}}),
    ];
    let wanted = artifacts::parse(&mut manifest, &files).unwrap();
    assert!(wanted[0].classpath);
    assert!(!wanted[1].classpath);
}

#[test]
fn generated_library_with_empty_url_is_not_dropped() {
    let mut value =
        json!({"libraries":[{"name":"g:generated:1","downloads":{"artifact":{"url":""}}}]});
    let wanted = artifacts::parse(&mut value, &[]).unwrap();
    assert_eq!(wanted.len(), 1);
    assert!(wanted[0].url.is_none());
}

#[test]
fn component_overrides_keep_platform_alternatives() {
    let mut out = components::Components::default();
    components::apply(&mut out, &json!({"libraries":[{"name":"g:a:1"}]})).unwrap();
    components::apply(&mut out,&json!({"libraries":[{"name":"g:a:2","rules":[{"action":"allow","os":{"name":"linux"}}]},{"name":"g:a:2","rules":[{"action":"allow","os":{"name":"windows"}}]}]})).unwrap();
    assert_eq!(out.manifest["libraries"].as_array().unwrap().len(), 2);
}

#[async_std::test]
async fn cycle_and_exact_component_conflicts_fail_without_http() {
    let temp = tempfile::tempdir().unwrap();
    let repo = Repository::new(temp.path().into());
    let locals = HashMap::from([
        (
            "a".into(),
            json!({"uid":"a","version":"1","requires":[{"uid":"b","equals":"1"}]}),
        ),
        (
            "b".into(),
            json!({"uid":"b","version":"1","requires":[{"uid":"a","equals":"1"}]}),
        ),
    ]);
    assert!(components::prism(
        &repo,
        "1.12.2",
        "https://example.invalid/",
        locals,
        vec![("a".into(), "1".into())]
    )
    .await
    .is_err());
}

#[async_std::test]
async fn installed_profile_roundtrips_through_actual_migration() {
    let db = crate::data::db::Database::init("sqlite::memory:")
        .await
        .unwrap();
    sqlx::query("INSERT INTO instances(id,name,dir,version,loader,manifest_url,meta_provider) VALUES(1,'test','test','1.20.1','vanilla','https://example.invalid/x','prism')").execute(&db.pool).await.unwrap();
    let mut profile = InstalledProfile {
        format_version: 1,
        minecraft: "1.20.1".into(),
        loader: LoaderSpec {
            kind: LoaderKind::Vanilla,
            version: None,
        },
        manifest: json!({}),
        java: JavaPolicy {
            majors: vec![17],
            allow_newer: false,
        },
        libraries_root: PathBuf::from("instances/1/generation/maven"),
        artifacts: vec![],
        metadata: vec![],
    };
    storage::publish(&db, 1.into(), &profile).await.unwrap();
    let actual = storage::load(&db, 1.into()).await.unwrap().unwrap();
    assert_eq!(actual.loader, profile.loader);
    assert_eq!(actual.libraries_root, profile.libraries_root);
    profile.manifest = json!({"updated":true});
    storage::publish(&db, 1.into(), &profile).await.unwrap();
    assert_eq!(
        storage::load(&db, 1.into())
            .await
            .unwrap()
            .unwrap()
            .manifest,
        profile.manifest
    );
}

#[async_std::test]
async fn cleanroom_release_registry_requires_real_digest() {
    let temp = tempfile::tempdir().unwrap();
    let catalog = ModloaderCatalog::new(temp.path().into());
    let bad = super::cleanroom::CleanroomRelease {
        version: "fixture".into(),
        minecraft: "1.12.2".into(),
        archive_url: "https://example.invalid/test.zip".into(),
        archive_sha256: "missing".into(),
        archive_root: String::new(),
        java: JavaPolicy {
            majors: vec![25],
            allow_newer: false,
        },
    };
    assert!(super::cleanroom::register(&catalog.repository, bad)
        .await
        .is_err());
    assert!(catalog
        .versions(LoaderKind::Cleanroom, "1.12.2")
        .await
        .unwrap()
        .is_empty());
}

#[test]
fn metadata_cache_identity_includes_the_entire_url() {
    assert_ne!(
        files::sha256(b"https://meta.fabricmc.net/v2/versions/loader/1.20.1/0.16/profile/json"),
        files::sha256(b"https://meta.fabricmc.net/v2/versions/loader/1.21.1/0.16/profile/json")
    );
}

#[test]
fn loader_override_replaces_all_old_platform_variants_and_keeps_new_variants() {
    let base = json!({"libraries":[{"name":"g:a:1","rules":[{"action":"allow","os":{"name":"linux"}}]},{"name":"g:a:1","rules":[{"action":"allow","os":{"name":"windows"}}]},{"name":"g:unrelated:1"}]});
    let patch = json!({"inheritsFrom":"1.20.1","libraries":[{"name":"g:a:2","rules":[{"action":"allow","os":{"name":"linux"}}]},{"name":"g:a:2","rules":[{"action":"allow","os":{"name":"windows"}}]}]});
    let merged = super::profile::merge_official(&base, &patch, "1.20.1").unwrap();
    assert_eq!(
        merged["libraries"],
        json!([patch["libraries"][0],patch["libraries"][1],{"name":"g:unrelated:1"}])
    );
    let universal = super::profile::merge_official(
        &base,
        &json!({"inheritsFrom":"1.20.1","libraries":[{"name":"g:a:2"}]}),
        "1.20.1",
    )
    .unwrap();
    assert_eq!(universal["libraries"].as_array().unwrap().len(), 2);
}

#[async_std::test]
async fn hydration_publishes_monotonic_file_progress_without_counting_generated_outputs() {
    use crate::{
        bus::{event::CoreEvent, EventBus},
        data::registry::operation::{
            message::{
                event::{OperationEvent, OperationUpdate},
                stage::OperationStage,
                status::{Outcome, Progress},
            },
            OperationRegistry,
        },
    };
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let mut wanted = Vec::new();
    for name in ["a.jar", "b.jar"] {
        let path = root.join(name);
        std::fs::write(&path, name).unwrap();
        wanted.push(artifacts::Wanted {
            path: name.into(),
            url: Some("not-requested".into()),
            sha1: Some(files::sha1_file(&path).unwrap()),
            classpath: true,
            native: false,
        });
    }
    wanted.push(artifacts::Wanted {
        path: "generated.jar".into(),
        url: None,
        sha1: None,
        classpath: true,
        native: false,
    });
    let bus = EventBus::new(32);
    let mut events = bus.subscribe();
    let registry = OperationRegistry::new(bus);
    let op = registry.begin("test".into(), vec![OperationStage::DownloadLibs]);
    artifacts::hydrate(
        &wanted,
        root,
        &root.join("blobs"),
        &transfer::Cancel::default(),
        &op,
    )
    .await
    .unwrap();
    let mut counts = Vec::new();
    while let Ok(event) = events.try_recv() {
        if let CoreEvent::Operation(msg) = event {
            if let OperationEvent::Update(OperationUpdate::Progress {
                progress: Progress::Determinable { current, total },
                ..
            }) = msg.data
            {
                counts.push((current, total));
            }
        }
    }
    assert_eq!(counts, vec![(0, 2), (1, 2), (2, 2)]);
    op.finish(Outcome::Completed);
}
