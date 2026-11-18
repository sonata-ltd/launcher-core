use std::collections::{HashMap, HashSet};

use crate::instance::content::model::{Candidate, DependencyKind, ProjectKey};

#[derive(Debug, Clone)]
pub struct Requirement {
    pub project: ProjectKey,
    pub exact_release: Option<String>,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ResolveError {
    #[error("content dependency conflict")]
    Conflict,
    #[error("content dependency search limit reached")]
    SearchLimit,
    #[error("unsupported content dependency relation")]
    UnsupportedRelation,
}

pub fn solve(
    roots: &[Requirement],
    candidates: &HashMap<ProjectKey, Vec<Candidate>>,
    max_decisions: usize,
) -> Result<HashMap<ProjectKey, Candidate>, ResolveError> {
    solve_with_installed(roots, candidates, HashMap::new(), max_decisions)
}

pub(in crate::instance::content) fn solve_with_installed(
    roots: &[Requirement],
    candidates: &HashMap<ProjectKey, Vec<Candidate>>,
    installed: HashMap<ProjectKey, Candidate>,
    max_decisions: usize,
) -> Result<HashMap<ProjectKey, Candidate>, ResolveError> {
    let mut decisions_left = max_decisions;
    let mut unsupported_seen = false;
    let mut requirements = roots.to_vec();
    let mut seen: HashSet<_> = requirements
        .iter()
        .map(|req| (req.project.clone(), req.exact_release.clone()))
        .collect();
    for requirement in installed_requirements(&installed)? {
        if seen.insert((
            requirement.project.clone(),
            requirement.exact_release.clone(),
        )) {
            requirements.push(requirement);
        }
    }

    let result = search(
        requirements,
        installed,
        candidates,
        &mut decisions_left,
        &mut unsupported_seen,
    )?;

    result.ok_or(if unsupported_seen {
        ResolveError::UnsupportedRelation
    } else {
        ResolveError::Conflict
    })
}

/// Fixed seeds must obey the same relation policy as newly selected candidates.
/// Stable order keeps bounded search and catalog traversal independent of HashMap order.
pub(super) fn installed_requirements(
    installed: &HashMap<ProjectKey, Candidate>,
) -> Result<Vec<Requirement>, ResolveError> {
    if installed
        .iter()
        .any(|(project, candidate)| *project != candidate.key.project)
    {
        return Err(ResolveError::Conflict);
    }
    let mut seeds: Vec<_> = installed.values().collect();
    seeds.sort_by(|left, right| {
        (
            left.key.project.provider.slug(),
            &left.key.project.project_id,
        )
            .cmp(&(
                right.key.project.provider.slug(),
                &right.key.project.project_id,
            ))
    });
    let mut requirements = Vec::new();
    let mut seen = HashSet::new();
    for candidate in seeds {
        if candidate
            .dependencies
            .iter()
            .any(|dependency| matches!(dependency.kind, DependencyKind::Unknown(_)))
        {
            return Err(ResolveError::UnsupportedRelation);
        }
        for dependency in &candidate.dependencies {
            if dependency.kind == DependencyKind::Required
                && seen.insert((dependency.project.clone(), dependency.exact_release.clone()))
            {
                requirements.push(Requirement {
                    project: dependency.project.clone(),
                    exact_release: dependency.exact_release.clone(),
                });
            }
        }
    }
    Ok(requirements)
}

fn search(
    requirements: Vec<Requirement>,
    selected: HashMap<ProjectKey, Candidate>,
    catalog: &HashMap<ProjectKey, Vec<Candidate>>,
    decisions_left: &mut usize,
    unsupported_seen: &mut bool,
) -> Result<Option<HashMap<ProjectKey, Candidate>>, ResolveError> {
    for requirement in &requirements {
        if let Some(candidate) = selected.get(&requirement.project) {
            if !satisfies(candidate, requirement) {
                return Ok(None);
            }
        }
    }

    if has_incompatibility(&selected) {
        return Ok(None);
    }

    let next = requirements
        .iter()
        .find(|requirement| !selected.contains_key(&requirement.project));

    let Some(next) = next else {
        return Ok(Some(selected));
    };

    let Some(available) = catalog.get(&next.project) else {
        return Ok(None);
    };

    for candidate in available {
        if candidate.key.project != next.project
            || requirements
                .iter()
                .any(|req| req.project == next.project && !satisfies(candidate, req))
        {
            continue;
        }

        if *decisions_left == 0 {
            return Err(ResolveError::SearchLimit);
        }

        *decisions_left -= 1;

        if candidate
            .dependencies
            .iter()
            .any(|dependency| matches!(dependency.kind, DependencyKind::Unknown(_)))
        {
            *unsupported_seen = true;
            continue;
        }

        let mut extended_requirements = requirements.clone();
        let mut seen: HashSet<_> = extended_requirements
            .iter()
            .map(|req| (req.project.clone(), req.exact_release.clone()))
            .collect();

        for dependency in &candidate.dependencies {
            if !matches!(dependency.kind, DependencyKind::Required) {
                continue;
            }

            let identity = (dependency.project.clone(), dependency.exact_release.clone());
            if seen.insert(identity) {
                extended_requirements.push(Requirement {
                    project: dependency.project.clone(),
                    exact_release: dependency.exact_release.clone(),
                });
            }
        }

        let mut extended_selected = selected.clone();
        extended_selected.insert(next.project.clone(), candidate.clone());

        if let Some(solution) = search(
            extended_requirements,
            extended_selected,
            catalog,
            decisions_left,
            unsupported_seen,
        )? {
            return Ok(Some(solution));
        }
    }

    Ok(None)
}

fn satisfies(candidate: &Candidate, requirement: &Requirement) -> bool {
    candidate.key.project == requirement.project
        && requirement
            .exact_release
            .as_ref()
            .is_none_or(|release| candidate.key.release_id == *release)
}

fn has_incompatibility(selected: &HashMap<ProjectKey, Candidate>) -> bool {
    selected.values().any(|candidate| {
        candidate.dependencies.iter().any(|dependency| {
            matches!(dependency.kind, DependencyKind::Incompatible)
                && selected.get(&dependency.project).is_some_and(|other| {
                    dependency
                        .exact_release
                        .as_ref()
                        .is_none_or(|release| other.key.release_id == *release)
                })
        })
    })
}
