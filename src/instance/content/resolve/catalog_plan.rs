//! HTTP-обогащение графа отделено от чистого backtracking solver.
use crate::instance::content::{
    catalog::{types::*, ContentCatalog, RequestBudget},
    error::{ContentError, Result},
    model::{Candidate, DependencyKind, ProjectKey},
    resolve::graph::{self, Requirement},
};
use std::collections::{HashMap, HashSet, VecDeque};

// A private seam for deterministic traversal tests; production still uses the bounded adapters.
trait ReleaseCatalog {
    async fn choices(
        &self,
        project: &ProjectKey,
        exact: Option<&str>,
        compatibility: &Compatibility,
        budget: &mut RequestBudget,
    ) -> Result<Vec<CatalogRelease>>;
}

impl ReleaseCatalog for ContentCatalog {
    async fn choices(
        &self,
        project: &ProjectKey,
        exact: Option<&str>,
        compatibility: &Compatibility,
        budget: &mut RequestBudget,
    ) -> Result<Vec<CatalogRelease>> {
        if let Some(exact) = exact {
            self.exact_release_bounded(
                project.provider,
                &project.project_id,
                exact,
                compatibility,
                budget,
            )
            .await
            .map(|release| vec![release])
        } else {
            self.releases_bounded(project, compatibility, budget).await
        }
    }
}

pub async fn resolve(
    catalog: &ContentCatalog,
    roots: &[ProjectKey],
    compatibility: &Compatibility,
) -> Result<Vec<CatalogRelease>> {
    resolve_with_installed(catalog, roots, compatibility, HashMap::new()).await
}

pub(in crate::instance::content) async fn resolve_with_installed(
    catalog: &ContentCatalog,
    roots: &[ProjectKey],
    compatibility: &Compatibility,
    installed: HashMap<ProjectKey, Candidate>,
) -> Result<Vec<CatalogRelease>> {
    resolve_catalog(catalog, roots, compatibility, installed).await
}

async fn resolve_catalog(
    catalog: &impl ReleaseCatalog,
    roots: &[ProjectKey],
    compatibility: &Compatibility,
    installed: HashMap<ProjectKey, Candidate>,
) -> Result<Vec<CatalogRelease>> {
    if roots.is_empty() || roots.len() > 16 {
        return Err(ContentError::Invalid(
            "root selection must contain 1 to 16 projects".into(),
        ));
    }
    if roots.iter().any(|root| root.provider != roots[0].provider) {
        return Err(ContentError::Invalid(
            "mixed-provider root selection is unsupported".into(),
        ));
    }
    let mut queue: VecDeque<_> = roots
        .iter()
        .cloned()
        .map(|project| (project, None::<String>))
        .collect();
    for requirement in graph::installed_requirements(&installed)? {
        queue.push_back((requirement.project, requirement.exact_release));
    }
    let mut visited = HashSet::new();
    let mut releases: HashMap<ProjectKey, Vec<CatalogRelease>> = HashMap::new();
    let mut budget = RequestBudget::default();
    while let Some((project, exact)) = queue.pop_front() {
        if installed.contains_key(&project) {
            continue;
        }
        if !visited.insert((project.clone(), exact.clone())) {
            continue;
        }
        if exact.as_ref().is_some_and(|id| {
            releases.get(&project).is_some_and(|available| {
                available
                    .iter()
                    .any(|release| release.key.release_id == *id)
            })
        }) {
            continue;
        }
        let choices = match catalog
            .choices(&project, exact.as_deref(), compatibility, &mut budget)
            .await
        {
            Ok(releases) => releases,
            Err(ContentError::CandidateUnavailable(reason)) if exact.is_some() => {
                tracing::debug!(%reason, "skipping unavailable exact release");
                continue;
            }
            Err(ContentError::Http(404)) if exact.is_some() || !roots.contains(&project) => {
                continue
            }
            Err(error) => return Err(error),
        };
        for release in choices {
            if exact
                .as_ref()
                .is_some_and(|id| release.key.release_id != *id)
            {
                return Err(ContentError::Invalid(
                    "provider returned a different exact release".into(),
                ));
            }
            if release.key.project != project {
                return Err(ContentError::Invalid(
                    "provider returned a release of another project".into(),
                ));
            }
            for dependency in &release.dependencies {
                if dependency.kind == DependencyKind::Required {
                    queue.push_back((dependency.project.clone(), dependency.exact_release.clone()));
                }
            }
            let versions = releases.entry(project.clone()).or_default();
            if !versions.iter().any(|existing| existing.key == release.key) {
                versions.push(release);
            }
        }
    }
    let candidates = releases
        .iter()
        .map(|(key, releases)| {
            (
                key.clone(),
                releases.iter().map(CatalogRelease::candidate).collect(),
            )
        })
        .collect();
    let requirements: Vec<_> = roots
        .iter()
        .cloned()
        .map(|project| Requirement {
            project,
            exact_release: None,
        })
        .collect();
    let installed_projects: HashSet<_> = installed.keys().cloned().collect();
    let selected = graph::solve_with_installed(&requirements, &candidates, installed, 4096)
        .map_err(ContentError::Incompatible)?;
    let mut result = Vec::new();
    for candidate in selected.into_values() {
        if installed_projects.contains(&candidate.key.project) {
            continue;
        }
        let release = releases
            .get(&candidate.key.project)
            .and_then(|versions| versions.iter().find(|release| release.key == candidate.key))
            .unwrap();
        result.push(release.clone());
    }
    result.sort_by(|left, right| left.file.file_name.cmp(&right.file.file_name));
    Ok(result)
}

#[cfg(test)]
#[path = "catalog_plan_tests.rs"]
mod tests;
