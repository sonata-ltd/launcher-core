//! HTTP-обогащение графа отделено от чистого backtracking solver.
use crate::instance::content::{
    catalog::{types::*, ContentCatalog, RequestBudget},
    error::{ContentError, Result},
    model::{DependencyKind, ProjectKey},
    resolve::graph::{self, Requirement},
};
use std::collections::{HashMap, HashSet, VecDeque};

pub async fn resolve(
    catalog: &ContentCatalog,
    roots: &[ProjectKey],
    compatibility: &Compatibility,
) -> Result<Vec<CatalogRelease>> {
    if roots.is_empty() || roots.len() > 16 {
        return Err(ContentError::Invalid("Выберите от 1 до 16 модов".into()));
    }
    if roots.iter().any(|root| root.provider != roots[0].provider) {
        return Err(ContentError::Invalid(
            "Установите выбор из разных провайдеров отдельными операциями".into(),
        ));
    }
    let mut queue: VecDeque<_> = roots
        .iter()
        .cloned()
        .map(|project| (project, None::<String>))
        .collect();
    let mut visited = HashSet::new();
    let mut releases: HashMap<ProjectKey, Vec<CatalogRelease>> = HashMap::new();
    let mut budget = RequestBudget::default();
    while let Some((project, exact)) = queue.pop_front() {
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
        let choices = if let Some(exact) = exact {
            match catalog
                .exact_release_bounded(
                    project.provider,
                    &project.project_id,
                    &exact,
                    compatibility,
                    &mut budget,
                )
                .await
            {
                Ok(release) if release.key.release_id == exact => vec![release],
                Ok(_) => {
                    return Err(ContentError::Invalid(
                        "provider returned a different exact release".into(),
                    ))
                }
                Err(ContentError::CandidateUnavailable(reason)) => {
                    tracing::debug!(%reason, "skipping unavailable exact release");
                    continue;
                }
                Err(ContentError::Http(404)) => continue,
                Err(error) => return Err(error),
            }
        } else {
            match catalog
                .releases_bounded(&project, compatibility, &mut budget)
                .await
            {
                Ok(releases) => releases,
                Err(ContentError::Http(404)) if !roots.contains(&project) => continue,
                Err(error) => return Err(error),
            }
        };
        for release in choices {
            if release.key.project != project {
                return Err(ContentError::Invalid(
                    "Провайдер вернул релиз другого проекта".into(),
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
    let selected = graph::solve(&requirements, &candidates, 4096).map_err(|error| {
        ContentError::Invalid(format!("Не удалось подобрать зависимости: {error:?}"))
    })?;
    let mut result = Vec::new();
    for candidate in selected.into_values() {
        let release = releases
            .get(&candidate.key.project)
            .and_then(|versions| versions.iter().find(|release| release.key == candidate.key))
            .unwrap();
        result.push(release.clone());
    }
    result.sort_by(|left, right| left.file.file_name.cmp(&right.file.file_name));
    Ok(result)
}
