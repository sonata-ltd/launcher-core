use super::*;
use crate::instance::content::{
    model::{Dependency, Provider, ReleaseKey},
    resolve::graph::ResolveError,
};
use std::sync::Mutex;

type Lookup = (ProjectKey, Option<String>);

#[derive(Default)]
struct FakeCatalog {
    releases: HashMap<Lookup, Vec<CatalogRelease>>,
    failures: HashMap<Lookup, u16>,
    calls: Mutex<Vec<Lookup>>,
}

impl ReleaseCatalog for FakeCatalog {
    async fn choices(
        &self,
        project: &ProjectKey,
        exact: Option<&str>,
        compatibility: &Compatibility,
        budget: &mut RequestBudget,
    ) -> Result<Vec<CatalogRelease>> {
        assert_eq!(compatibility.minecraft, "1.21.1");
        assert_eq!(compatibility.loader, "fabric");
        budget.charge_request()?;
        let key = (project.clone(), exact.map(str::to_owned));
        self.calls.lock().unwrap().push(key.clone());
        if let Some(status) = self.failures.get(&key) {
            return Err(ContentError::Http(*status));
        }
        self.releases
            .get(&key)
            .cloned()
            .ok_or(ContentError::Http(404))
    }
}

fn project(name: &str) -> ProjectKey {
    ProjectKey {
        provider: Provider::Modrinth,
        project_id: name.into(),
    }
}

fn required(name: &str, exact: Option<&str>) -> Dependency {
    Dependency {
        project: project(name),
        exact_release: exact.map(str::to_owned),
        kind: DependencyKind::Required,
    }
}

fn release(name: &str, version: &str, dependencies: Vec<Dependency>) -> CatalogRelease {
    CatalogRelease {
        key: ReleaseKey {
            project: project(name),
            release_id: version.into(),
        },
        title: name.into(),
        file: DownloadFile {
            file_name: format!("{name}.jar"),
            url: "https://example.invalid/unused.jar".into(),
            size_bytes: 1,
            sha1: Some("0".repeat(40)),
            sha512: None,
        },
        dependencies,
    }
}

fn compatibility() -> Compatibility {
    Compatibility {
        minecraft: "1.21.1".into(),
        loader: "fabric".into(),
    }
}

#[async_std::test]
async fn installed_catalog_traverses_exact_and_transitive_edges_without_fetching_seeds() {
    let seed = release(
        "installed",
        "fixed",
        vec![
            required("dependency", Some("1")),
            required("dependency", Some("1")),
        ],
    );
    let catalog = FakeCatalog {
        releases: HashMap::from([
            ((project("root"), None), vec![release("root", "1", vec![])]),
            (
                (project("dependency"), Some("1".into())),
                vec![release(
                    "dependency",
                    "1",
                    vec![required("transitive", None)],
                )],
            ),
            (
                (project("transitive"), None),
                vec![release(
                    "transitive",
                    "1",
                    vec![required("installed", Some("fixed"))],
                )],
            ),
        ]),
        ..Default::default()
    };
    let selected = resolve_catalog(
        &catalog,
        &[project("root")],
        &compatibility(),
        HashMap::from([(project("installed"), seed.candidate())]),
    )
    .await
    .unwrap();
    assert_eq!(selected.len(), 3);
    assert_eq!(
        catalog.calls.lock().unwrap().as_slice(),
        &[
            (project("root"), None),
            (project("dependency"), Some("1".into())),
            (project("transitive"), None),
        ]
    );
    assert!(selected
        .iter()
        .all(|item| item.key.project != project("installed")));
}

#[async_std::test]
async fn installed_catalog_rejects_unknown_relations_before_any_lookup() {
    let mut dependency = required("unknown", None);
    dependency.kind = DependencyKind::Unknown("future".into());
    let seed = release("installed", "fixed", vec![dependency]);
    let catalog = FakeCatalog::default();
    assert!(matches!(
        resolve_catalog(
            &catalog,
            &[project("root")],
            &compatibility(),
            HashMap::from([(project("installed"), seed.candidate())])
        )
        .await,
        Err(ContentError::Incompatible(
            ResolveError::UnsupportedRelation
        ))
    ));
    assert!(catalog.calls.lock().unwrap().is_empty());
}

#[async_std::test]
async fn installed_catalog_keeps_exact_constraints_on_other_fixed_seeds() {
    let a = release("a", "1", vec![required("b", Some("1"))]);
    let b = release("b", "2", vec![]);
    let catalog = FakeCatalog::default();
    assert!(matches!(
        resolve_catalog(
            &catalog,
            &[project("a")],
            &compatibility(),
            HashMap::from([(project("a"), a.candidate()), (project("b"), b.candidate())])
        )
        .await,
        Err(ContentError::Incompatible(ResolveError::Conflict))
    ));
    assert!(catalog.calls.lock().unwrap().is_empty());
}

#[async_std::test]
async fn installed_catalog_missing_required_release_is_a_conflict_not_success() {
    let seed = release("installed", "fixed", vec![required("missing", Some("1"))]);
    let catalog = FakeCatalog::default();
    assert!(matches!(
        resolve_catalog(
            &catalog,
            &[project("installed")],
            &compatibility(),
            HashMap::from([(project("installed"), seed.candidate())])
        )
        .await,
        Err(ContentError::Incompatible(ResolveError::Conflict))
    ));
    assert_eq!(catalog.calls.lock().unwrap().len(), 1);
}

#[async_std::test]
async fn installed_catalog_provider_failure_is_not_a_dependency_conflict() {
    let seed = release(
        "installed",
        "fixed",
        vec![required("dependency", Some("1"))],
    );
    let catalog = FakeCatalog {
        failures: HashMap::from([((project("dependency"), Some("1".into())), 503)]),
        ..Default::default()
    };
    assert!(matches!(
        resolve_catalog(
            &catalog,
            &[project("installed")],
            &compatibility(),
            HashMap::from([(project("installed"), seed.candidate())])
        )
        .await,
        Err(ContentError::Http(503))
    ));
}

#[async_std::test]
async fn installed_catalog_enforces_one_shared_request_budget() {
    let deps = (0..129)
        .map(|i| required(&format!("dep-{i}"), None))
        .collect();
    let seed = release("installed", "fixed", deps);
    let catalog = FakeCatalog {
        releases: (0..129)
            .map(|i| {
                let name = format!("dep-{i}");
                ((project(&name), None), vec![release(&name, "1", vec![])])
            })
            .collect(),
        ..Default::default()
    };
    assert!(matches!(
        resolve_catalog(
            &catalog,
            &[project("installed")],
            &compatibility(),
            HashMap::from([(project("installed"), seed.candidate())])
        )
        .await,
        Err(ContentError::CatalogRequestLimit(128))
    ));
    assert_eq!(catalog.calls.lock().unwrap().len(), 128);
}

#[async_std::test]
async fn installed_catalog_backtracks_with_a_later_exact_constraint() {
    let seed = release(
        "installed",
        "fixed",
        vec![required("b", None), required("c", None)],
    );
    let catalog = FakeCatalog {
        releases: HashMap::from([
            (
                (project("b"), None),
                vec![release("b", "2", vec![]), release("b", "1", vec![])],
            ),
            (
                (project("c"), None),
                vec![release("c", "1", vec![required("b", Some("1"))])],
            ),
        ]),
        ..Default::default()
    };
    let selected = resolve_catalog(
        &catalog,
        &[project("installed")],
        &compatibility(),
        HashMap::from([(project("installed"), seed.candidate())]),
    )
    .await
    .unwrap();
    assert_eq!(
        selected
            .iter()
            .find(|r| r.key.project == project("b"))
            .unwrap()
            .key
            .release_id,
        "1"
    );
    assert_eq!(catalog.calls.lock().unwrap().len(), 2);
}

#[async_std::test]
async fn installed_catalog_checks_exact_release_and_project_identity() {
    let seed = release(
        "installed",
        "fixed",
        vec![required("dependency", Some("1"))],
    );
    for invalid in [
        release("dependency", "wrong", vec![]),
        release("wrong-project", "1", vec![]),
    ] {
        let catalog = FakeCatalog {
            releases: HashMap::from([((project("dependency"), Some("1".into())), vec![invalid])]),
            ..Default::default()
        };
        assert!(matches!(
            resolve_catalog(
                &catalog,
                &[project("installed")],
                &compatibility(),
                HashMap::from([(project("installed"), seed.candidate())])
            )
            .await,
            Err(ContentError::Invalid(_))
        ));
    }
}
