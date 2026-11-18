//! Installed-aware planning regressions, real SQLite/files and no provider HTTP.
use super::{fixture, prepared, Fixture};
use crate::instance::content::{
    error::ContentError,
    journal::{Placement, PublishedFile},
    model::{Dependency, DependencyKind, ProjectKey, Provider},
    resolve::graph::ResolveError,
    service::InstanceContentService,
    storage,
    store::{storage as objects, ObjectStore},
};
use crate::utils::cancellation::Cancellation;

async fn service(fixture: &Fixture) -> InstanceContentService {
    InstanceContentService::new(
        fixture.db.clone(),
        fixture.paths.clone(),
        fixture.instances.clone(),
        None,
        fixture.operations.clone(),
    )
    .await
    .unwrap()
}

async fn register_mod(
    fixture: &Fixture,
    project: ProjectKey,
    release: &str,
    name: &str,
    bytes: &[u8],
    dependencies: Option<Vec<Dependency>>,
) {
    let source = fixture.root.join("source.jar");
    std::fs::write(&source, bytes).unwrap();
    let store = ObjectStore::new(fixture.paths.root().join("content-store"));
    let object = store
        .import_file(&source, None, &Cancellation::default())
        .unwrap();
    let object_id = objects::register(&fixture.db, &object).await.unwrap();
    let files = vec![PublishedFile {
        name: name.into(),
        sha256: object.digests.sha256,
        object_id,
        provider: project.provider,
        project_id: project.project_id,
        release_id: release.into(),
        placement: Placement::Copy,
        dependencies,
    }];
    let operation = uuid::Uuid::new_v4();
    storage::prepare(&fixture.db, operation, fixture.id, &files)
        .await
        .unwrap();
    std::fs::write(fixture.root.join("mods").join(name), bytes).unwrap();
    storage::complete(&fixture.db, operation, fixture.id, &files)
        .await
        .unwrap();
}

fn project(name: &str) -> ProjectKey {
    ProjectKey {
        provider: Provider::Modrinth,
        project_id: name.into(),
    }
}

#[async_std::test]
async fn installed_missing_dependency_metadata_is_not_an_empty_list() {
    let fixture = fixture().await;
    let (operation, files) = prepared(&fixture).await;
    std::fs::write(fixture.root.join("mods/mod.jar"), b"verified bytes").unwrap();
    storage::complete(&fixture.db, operation, fixture.id, &files)
        .await
        .unwrap();
    let registered = storage::registered_mods(&fixture.db, fixture.id)
        .await
        .unwrap();
    assert!(registered[0].dependencies_json.is_none());
    assert!(registered[0].dependencies().is_err());
    register_mod(
        &fixture,
        project("unrelated-root"),
        "1",
        "root.jar",
        b"root bytes",
        Some(vec![]),
    )
    .await;

    let service = InstanceContentService::new(
        fixture.db.clone(),
        fixture.paths.clone(),
        fixture.instances.clone(),
        None,
        fixture.operations.clone(),
    )
    .await
    .unwrap();
    for root in [registered[0].project(), project("unrelated-root")] {
        let result = service.plan_install(fixture.id, vec![root]).await;
        assert!(
            matches!(result, Err(ContentError::MissingDependencyMetadata(key))
            if key.project == registered[0].project() && key.release_id == registered[0].release_id)
        );
    }
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM content_provider_files WHERE dependencies_json IS NULL",
        )
        .fetch_one(&fixture.db.pool)
        .await
        .unwrap(),
        1
    );
    drop(service);
    fixture.operations.tasks.shutdown().await;
}

#[async_std::test]
async fn installed_explicit_empty_dependencies_allow_an_already_present_preview() {
    let fixture = fixture().await;
    let (operation, mut files) = prepared(&fixture).await;
    files[0].dependencies = Some(Vec::new());
    std::fs::write(fixture.root.join("mods/mod.jar"), b"verified bytes").unwrap();
    storage::complete(&fixture.db, operation, fixture.id, &files)
        .await
        .unwrap();
    let registered = storage::registered_mods(&fixture.db, fixture.id)
        .await
        .unwrap();
    assert!(registered[0].dependencies().unwrap().is_empty());
    let service = InstanceContentService::new(
        fixture.db.clone(),
        fixture.paths.clone(),
        fixture.instances.clone(),
        None,
        fixture.operations.clone(),
    )
    .await
    .unwrap();
    let preview = service
        .plan_install(fixture.id, vec![registered[0].project()])
        .await
        .unwrap();
    assert_eq!(preview.download_bytes, 0);
    assert_eq!(preview.items.len(), 1);
    assert!(preview.items[0].already_installed);
    drop(service);
    fixture.operations.tasks.shutdown().await;
}

#[async_std::test]
async fn installed_corrupt_dependency_metadata_is_not_an_empty_list() {
    let fixture = fixture().await;
    let (operation, files) = prepared(&fixture).await;
    std::fs::write(fixture.root.join("mods/mod.jar"), b"verified bytes").unwrap();
    storage::complete(&fixture.db, operation, fixture.id, &files)
        .await
        .unwrap();
    let service = InstanceContentService::new(
        fixture.db.clone(),
        fixture.paths.clone(),
        fixture.instances.clone(),
        None,
        fixture.operations.clone(),
    )
    .await
    .unwrap();
    let registered = storage::registered_mods(&fixture.db, fixture.id)
        .await
        .unwrap();
    for invalid in ["not-json", "null", "{}", "[{\"kind\":\"Required\"}]"] {
        sqlx::query("UPDATE content_provider_files SET dependencies_json=?")
            .bind(invalid)
            .execute(&fixture.db.pool)
            .await
            .unwrap();
        assert!(
            matches!(
                service
                    .plan_install(fixture.id, vec![registered[0].project()])
                    .await,
                Err(ContentError::Json(_))
            ),
            "{invalid}"
        );
    }
    drop(service);
    fixture.operations.tasks.shutdown().await;
}

#[async_std::test]
async fn installed_verified_unknown_relation_fails_before_provider_http() {
    let fixture = fixture().await;
    register_mod(
        &fixture,
        project("installed"),
        "fixed",
        "mod.jar",
        b"verified bytes",
        Some(vec![Dependency {
            project: project("unknown"),
            exact_release: None,
            kind: DependencyKind::Unknown("future".into()),
        }]),
    )
    .await;
    let service = service(&fixture).await;
    assert!(matches!(
        service
            .plan_install(fixture.id, vec![project("installed")])
            .await,
        Err(ContentError::Incompatible(
            ResolveError::UnsupportedRelation
        ))
    ));
    drop(service);
    fixture.operations.tasks.shutdown().await;
}

#[async_std::test]
async fn installed_fixed_dependency_conflict_is_checked_by_service() {
    let fixture = fixture().await;
    register_mod(
        &fixture,
        project("a"),
        "1",
        "a.jar",
        b"a bytes",
        Some(vec![Dependency {
            project: project("b"),
            exact_release: Some("1".into()),
            kind: DependencyKind::Required,
        }]),
    )
    .await;
    register_mod(
        &fixture,
        project("b"),
        "2",
        "b.jar",
        b"b bytes",
        Some(vec![]),
    )
    .await;
    let service = service(&fixture).await;
    assert!(matches!(
        service.plan_install(fixture.id, vec![project("a")]).await,
        Err(ContentError::Incompatible(ResolveError::Conflict))
    ));
    drop(service);
    fixture.operations.tasks.shutdown().await;
}

#[async_std::test]
async fn installed_modified_disabled_missing_and_ambiguous_files_do_not_seed_metadata() {
    for state in ["modified", "disabled", "missing", "ambiguous"] {
        let fixture = fixture().await;
        register_mod(
            &fixture,
            project("historical"),
            "old",
            "old.jar",
            b"old bytes",
            None,
        )
        .await;
        register_mod(
            &fixture,
            project("root"),
            "1",
            "root.jar",
            b"root bytes",
            Some(vec![]),
        )
        .await;
        let old = fixture.root.join("mods/old.jar");
        match state {
            "modified" => std::fs::write(&old, b"changed bytes").unwrap(),
            "disabled" => {
                std::fs::rename(&old, fixture.root.join("mods/old.jar.disabled")).unwrap()
            }
            "missing" => std::fs::remove_file(&old).unwrap(),
            "ambiguous" => {
                std::fs::rename(&old, fixture.root.join("mods/alias-a.jar")).unwrap();
                std::fs::write(fixture.root.join("mods/alias-b.jar"), b"old bytes").unwrap();
            }
            _ => unreachable!(),
        }
        let service = service(&fixture).await;
        let preview = service
            .plan_install(fixture.id, vec![project("root")])
            .await
            .unwrap();
        assert_eq!(preview.items.len(), 1, "{state}");
        assert!(preview.items[0].already_installed);
        let mut edge = Dependency {
            project: project("must-not-fetch"),
            exact_release: None,
            kind: DependencyKind::Required,
        };
        let required = serde_json::to_string(&vec![edge.clone()]).unwrap();
        edge.kind = DependencyKind::Unknown("future".into());
        let unknown = serde_json::to_string(&vec![edge]).unwrap();
        for metadata in ["not-json".to_owned(), required, unknown] {
            sqlx::query("UPDATE content_provider_files SET dependencies_json=? WHERE project_id='historical'")
                .bind(&metadata).execute(&fixture.db.pool).await.unwrap();
            assert_eq!(
                service
                    .plan_install(fixture.id, vec![project("root")])
                    .await
                    .unwrap()
                    .items
                    .len(),
                1,
                "{state}/{metadata}"
            );
        }
        drop(service);
        fixture.operations.tasks.shutdown().await;
    }
}

#[async_std::test]
async fn installed_unique_verified_rename_keeps_required_constraints() {
    let fixture = fixture().await;
    register_mod(
        &fixture,
        project("a"),
        "1",
        "a.jar",
        b"a bytes",
        Some(vec![Dependency {
            project: project("b"),
            exact_release: Some("1".into()),
            kind: DependencyKind::Required,
        }]),
    )
    .await;
    register_mod(
        &fixture,
        project("b"),
        "1",
        "b.jar",
        b"b bytes",
        Some(vec![]),
    )
    .await;
    std::fs::rename(
        fixture.root.join("mods/a.jar"),
        fixture.root.join("mods/renamed.jar"),
    )
    .unwrap();
    let service = service(&fixture).await;
    let preview = service
        .plan_install(fixture.id, vec![project("a")])
        .await
        .unwrap();
    assert_eq!(preview.items.len(), 1);
    assert_eq!(preview.items[0].file_name, "renamed.jar");
    assert_eq!(preview.download_bytes, 0);
    drop(service);
    fixture.operations.tasks.shutdown().await;
}

#[async_std::test]
async fn installed_multiple_active_releases_are_not_silently_overwritten() {
    let fixture = fixture().await;
    register_mod(
        &fixture,
        project("a"),
        "1",
        "a.jar",
        b"a bytes",
        Some(vec![]),
    )
    .await;
    register_mod(
        &fixture,
        project("a"),
        "2",
        "other.jar",
        b"other bytes",
        Some(vec![]),
    )
    .await;
    let service = service(&fixture).await;
    assert!(matches!(
        service.plan_install(fixture.id, vec![project("a")]).await,
        Err(ContentError::Invalid(_))
    ));
    drop(service);
    fixture.operations.tasks.shutdown().await;
}

#[async_std::test]
async fn installed_same_release_with_conflicting_metadata_is_not_silently_overwritten() {
    let fixture = fixture().await;
    register_mod(
        &fixture,
        project("a"),
        "1",
        "a.jar",
        b"a bytes",
        Some(vec![]),
    )
    .await;
    register_mod(
        &fixture,
        project("a"),
        "1",
        "other.jar",
        b"other bytes",
        Some(vec![Dependency {
            project: project("b"),
            exact_release: None,
            kind: DependencyKind::Required,
        }]),
    )
    .await;
    let service = service(&fixture).await;
    assert!(matches!(
        service.plan_install(fixture.id, vec![project("a")]).await,
        Err(ContentError::Invalid(_))
    ));
    drop(service);
    fixture.operations.tasks.shutdown().await;
}

#[async_std::test]
async fn installed_identical_release_and_metadata_bindings_remain_deduplicated() {
    let fixture = fixture().await;
    register_mod(
        &fixture,
        project("a"),
        "1",
        "a.jar",
        b"a bytes",
        Some(vec![]),
    )
    .await;
    register_mod(
        &fixture,
        project("a"),
        "1",
        "other.jar",
        b"other bytes",
        Some(vec![]),
    )
    .await;
    let service = service(&fixture).await;
    let preview = service
        .plan_install(fixture.id, vec![project("a")])
        .await
        .unwrap();
    assert_eq!(preview.items.len(), 1);
    assert_eq!(preview.download_bytes, 0);
    drop(service);
    fixture.operations.tasks.shutdown().await;
}
