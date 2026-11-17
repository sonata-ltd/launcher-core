mod dto;
pub(super) use dto::Version as VersionResponse;
use std::sync::Arc;

use super::types::*;
use crate::{
    instance::content::{
        error::{ContentError, Result},
        model::{Dependency, DependencyKind, Provider, ReleaseKey},
    },
    utils::download::json,
};

pub struct Modrinth {
    client: json::JsonClient,
}

impl Modrinth {
    pub fn new() -> Self {
        Self {
            client: json::JsonClient::default(),
        }
    }

    fn request(&self, url: surf::Url) -> Result<surf::RequestBuilder> {
        Ok(self.client.get(url)?)
    }

    fn url(path: &[&str]) -> Result<surf::Url> {
        let mut url = surf::Url::parse("https://api.modrinth.com/v2/").unwrap();
        url.path_segments_mut()
            .map_err(|_| ContentError::Invalid("Invalid API base URL".into()))?
            .pop_if_empty()
            .extend(path.iter().copied());
        Ok(url)
    }

    pub async fn search(
        &self,
        query: &str,
        offset: u32,
        compatibility: &Compatibility,
    ) -> Result<SearchPage> {
        let mut url = Self::url(&["search"])?;
        let facets = serde_json::json!([
            ["project_type:mod"],
            [format!("versions:{}", compatibility.minecraft)],
            [format!("categories:{}", compatibility.loader)]
        ])
        .to_string();
        url.query_pairs_mut()
            .append_pair("query", query)
            .append_pair("facets", &facets)
            .append_pair("limit", "20")
            .append_pair("offset", &offset.to_string());
        let response: dto::Search = json::get(self.request(url)?).await?;
        Ok(SearchPage {
            offset,
            total: response.total_hits,
            projects: response
                .hits
                .into_iter()
                .map(|hit| CatalogProject {
                    key: project_key(Provider::Modrinth, hit.project_id),
                    title: hit.title,
                    summary: hit.description,
                    author: hit.author,
                    downloads: hit.downloads,
                })
                .collect(),
        })
    }

    pub async fn releases(
        &self,
        project: &str,
        compatibility: &Compatibility,
    ) -> Result<Vec<CatalogRelease>> {
        self.releases_bounded(project, compatibility, &mut super::RequestBudget::default())
            .await
    }

    pub(super) async fn releases_bounded(
        &self,
        project: &str,
        compatibility: &Compatibility,
        budget: &mut super::RequestBudget,
    ) -> Result<Vec<CatalogRelease>> {
        let mut url = Self::url(&["project", project, "version"])?;
        url.query_pairs_mut()
            .append_pair(
                "game_versions",
                &serde_json::json!([compatibility.minecraft]).to_string(),
            )
            .append_pair(
                "loaders",
                &serde_json::json!([compatibility.loader]).to_string(),
            );
        let mut versions: Vec<dto::Version> = budget.get(self.request(url)?).await?;
        versions.retain(|version| compatible(version, compatibility));
        versions.sort_by(|left, right| right.date_published.cmp(&left.date_published));
        self.normalize_releases(versions, compatibility, budget)
            .await
    }

    async fn normalize_releases(
        &self,
        versions: Vec<dto::Version>,
        compatibility: &Compatibility,
        budget: &mut super::RequestBudget,
    ) -> Result<Vec<CatalogRelease>> {
        let mut releases = Vec::new();
        for version in versions.into_iter().take(64) {
            match self.normalize(&version, compatibility, budget).await {
                Ok(release) => releases.push(release),
                Err(ContentError::CandidateUnavailable(reason)) => {
                    tracing::debug!(%reason, "skipping unavailable Modrinth release")
                }
                Err(error) => return Err(error),
            }
            if releases.len() == 16 {
                break;
            }
        }
        Ok(releases)
    }

    pub async fn exact_release(
        &self,
        release: &str,
        compatibility: &Compatibility,
    ) -> Result<CatalogRelease> {
        self.exact_release_bounded(release, compatibility, &mut super::RequestBudget::default())
            .await
    }

    pub(super) async fn exact_release_bounded(
        &self,
        release: &str,
        compatibility: &Compatibility,
        budget: &mut super::RequestBudget,
    ) -> Result<CatalogRelease> {
        let version = self.version(release, budget).await?;
        if !compatible(&version, compatibility) {
            return Err(ContentError::CandidateUnavailable(format!(
                "Точная зависимость {} не подходит экземпляру",
                version.name
            )));
        }
        self.normalize(&version, compatibility, budget).await
    }

    async fn version(
        &self,
        id: &str,
        budget: &mut super::RequestBudget,
    ) -> Result<Arc<dto::Version>> {
        let unavailable =
            || ContentError::CandidateUnavailable(format!("Modrinth release {id} was not found"));
        if let Some(cached) = budget.cached_modrinth_version(id) {
            return cached.ok_or_else(unavailable);
        }
        let result: Result<dto::Version> = budget
            .get(self.request(Self::url(&["version", id])?)?)
            .await;
        match result {
            Ok(version) if version.id == id => {
                let version = Arc::new(version);
                budget.remember_modrinth_version(id.to_owned(), Some(version.clone()));
                Ok(version)
            }
            Ok(_) => Err(ContentError::Invalid(
                "Modrinth returned a different exact release".into(),
            )),
            Err(ContentError::Http(404)) => {
                budget.remember_modrinth_version(id.to_owned(), None);
                Err(unavailable())
            }
            Err(error) => Err(error),
        }
    }

    async fn normalize(
        &self,
        version: &dto::Version,
        compatibility: &Compatibility,
        budget: &mut super::RequestBudget,
    ) -> Result<CatalogRelease> {
        if version.dependencies.len() > 64 {
            return Err(ContentError::CandidateUnavailable(
                "Релиз содержит слишком много зависимостей".into(),
            ));
        }
        let selected = select_file(&version.files).ok_or_else(|| {
            ContentError::CandidateUnavailable(format!(
                "В {} нет устанавливаемого JAR",
                version.name
            ))
        })?;
        let file = DownloadFile {
            file_name: selected.filename.clone(),
            url: selected.url.clone(),
            size_bytes: selected.size,
            sha1: selected.hashes.sha1.clone(),
            sha512: selected.hashes.sha512.clone(),
        };
        file.validate()?;
        let mut dependencies = Vec::new();
        for dependency in &version.dependencies {
            let kind = match dependency.dependency_type.as_str() {
                "required" => DependencyKind::Required,
                "optional" => DependencyKind::Optional,
                "incompatible" => DependencyKind::Incompatible,
                "embedded" => DependencyKind::Embedded,
                other => DependencyKind::Unknown(other.to_owned()),
            };
            if matches!(kind, DependencyKind::Embedded | DependencyKind::Optional) {
                continue;
            }
            let project = if let Some(project) = &dependency.project_id {
                project.clone()
            } else if let Some(exact) = &dependency.version_id {
                // version-only dependency: получаем проект, не угадываем его
                // по имени файла. Этот запрос не обходит граф рекурсивно.
                let target = self.version(exact, budget).await?;
                if matches!(kind, DependencyKind::Required) && !compatible(&target, compatibility) {
                    return Err(ContentError::CandidateUnavailable(
                        "Точная зависимость несовместима с экземпляром".into(),
                    ));
                }
                target.project_id.clone()
            } else {
                return Err(ContentError::CandidateUnavailable(
                    "Зависимость без project_id/version_id требует ручной установки".into(),
                ));
            };
            dependencies.push(Dependency {
                project: project_key(Provider::Modrinth, project),
                exact_release: dependency.version_id.clone(),
                kind,
            });
        }
        Ok(CatalogRelease {
            key: ReleaseKey {
                project: project_key(Provider::Modrinth, version.project_id.clone()),
                release_id: version.id.clone(),
            },
            title: version.name.clone(),
            dependencies,
            file,
        })
    }
}

fn compatible(version: &dto::Version, compatibility: &Compatibility) -> bool {
    // Первая поставка автоматически выбирает только стабильные релизы.
    !matches!(
        version.environment.as_deref(),
        Some("server_only" | "server_only_client_optional" | "dedicated_server_only")
    ) && version.version_type == "release"
        && version.game_versions.contains(&compatibility.minecraft)
        && version.loaders.contains(&compatibility.loader)
}

fn select_file(files: &[dto::File]) -> Option<&dto::File> {
    let installable = |file: &&dto::File| {
        file.filename.to_ascii_lowercase().ends_with(".jar")
            && !matches!(
                file.file_type.as_deref(),
                Some("sources-jar" | "dev-jar" | "javadoc-jar" | "signature")
            )
    };
    files
        .iter()
        .filter(installable)
        .find(|file| file.primary)
        .or_else(|| files.iter().find(installable))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[async_std::test]
    async fn a_release_with_an_unresolvable_dependency_does_not_hide_alternatives() {
        let version = serde_json::json!({
            "id": "valid", "project_id": "project", "name": "Example",
            "version_type": "release", "date_published": "2026-01-01",
            "game_versions": ["1.21.1"], "loaders": ["fabric"],
            "files": [{
                "filename": "mod.jar", "url": "https://example.invalid/mod.jar",
                "size": 3, "primary": true, "hashes": {"sha512": "a".repeat(128)}
            }]
        });
        let mut invalid = version.clone();
        invalid["id"] = serde_json::json!("invalid");
        invalid["dependencies"] = serde_json::json!([{"dependency_type": "required"}]);
        let versions = serde_json::from_value(serde_json::json!([invalid, version])).unwrap();
        let compatibility = Compatibility {
            minecraft: "1.21.1".into(),
            loader: "fabric".into(),
        };
        let releases = Modrinth::new()
            .normalize_releases(
                versions,
                &compatibility,
                &mut super::super::RequestBudget::default(),
            )
            .await
            .unwrap();
        assert_eq!(releases.len(), 1);
        assert_eq!(releases[0].key.release_id, "valid");
    }

    #[test]
    fn sources_are_never_installed_as_the_primary_mod() {
        let files: Vec<dto::File> = serde_json::from_value(serde_json::json!([
            {"filename":"sources.jar","url":"https://cdn.modrinth.com/sources.jar","size":1,"primary":true,"file_type":"sources-jar","hashes":{}},
            {"filename":"mod.jar","url":"https://cdn.modrinth.com/mod.jar","size":2,"primary":false,"file_type":null,"hashes":{}}
        ])).unwrap();
        assert_eq!(select_file(&files).unwrap().filename, "mod.jar");
    }
    #[test]
    fn url_segments_cannot_escape_into_a_query_or_another_host() {
        let url = Modrinth::url(&["project", "a/b?token=bad", "version"]).unwrap();
        assert_eq!(url.host_str(), Some("api.modrinth.com"));
        assert_eq!(Modrinth::url(&["search"]).unwrap().path(), "/v2/search");
        assert!(url.query().is_none());
    }
}
