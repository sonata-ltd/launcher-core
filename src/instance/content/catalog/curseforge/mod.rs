mod dto;
pub mod mapping;

use super::types::*;
use crate::instance::content::{
    error::{ContentError, Result},
    model::{Dependency, Provider, ReleaseKey},
};
use crate::utils::download::json;

pub struct CurseForge {
    client: json::JsonClient,
    key: Option<String>,
}

impl CurseForge {
    pub fn new(key: Option<String>) -> Self {
        Self {
            client: json::JsonClient::default(),
            key: key.filter(|value| !value.trim().is_empty()),
        }
    }

    fn request(&self, url: surf::Url) -> Result<surf::RequestBuilder> {
        let key = self
            .key
            .as_deref()
            .ok_or(ContentError::MissingCurseForgeKey)?;
        Ok(self.client.get(url)?.header("x-api-key", key))
    }

    fn url(path: &[&str]) -> surf::Url {
        let mut url = surf::Url::parse("https://api.curseforge.com/v1/").unwrap();
        url.path_segments_mut()
            .unwrap()
            .pop_if_empty()
            .extend(path.iter().copied());
        url
    }

    pub async fn search(
        &self,
        query: &str,
        offset: u32,
        compatibility: &Compatibility,
    ) -> Result<SearchPage> {
        let mut url = Self::url(&["mods", "search"]);
        url.query_pairs_mut()
            .append_pair("gameId", "432")
            .append_pair("classId", "6")
            .append_pair("searchFilter", query)
            .append_pair("gameVersion", &compatibility.minecraft)
            .append_pair("modLoaderType", loader_id(&compatibility.loader)?)
            .append_pair("index", &offset.to_string())
            .append_pair("pageSize", "20");

        let response: dto::Search = json::get(self.request(url)?).await?;

        Ok(SearchPage {
            offset,
            total: response.pagination.total_count,
            projects: response
                .data
                .into_iter()
                .map(|project| CatalogProject {
                    key: project_key(Provider::CurseForge, project.id.to_string()),
                    title: project.name,
                    summary: project.summary,
                    author: project
                        .authors
                        .into_iter()
                        .map(|author| author.name)
                        .collect::<Vec<_>>()
                        .join(", "),
                    downloads: project.download_count.max(0.0) as u64,
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
        let mut versions = Vec::new();
        for page in 0..MAX_FILE_PAGES {
            let files = self
                .files_page(project, compatibility, page, budget)
                .await?;
            let done = files.len() < FILE_PAGE_SIZE;
            versions.extend(
                files
                    .into_iter()
                    .filter(|file| eligible(file, project, compatibility)),
            );
            if done {
                break;
            }
        }
        versions.sort_by(|left, right| right.file_date.cmp(&left.file_date));
        available_releases(versions)
    }

    pub async fn exact_release(
        &self,
        project: &str,
        release: &str,
        compatibility: &Compatibility,
    ) -> Result<CatalogRelease> {
        self.exact_release_bounded(
            project,
            release,
            compatibility,
            &mut super::RequestBudget::default(),
        )
        .await
    }

    pub(super) async fn exact_release_bounded(
        &self,
        project: &str,
        release: &str,
        compatibility: &Compatibility,
        budget: &mut super::RequestBudget,
    ) -> Result<CatalogRelease> {
        // Only the filtered files endpoint confirms loader compatibility.
        for page in 0..MAX_FILE_PAGES {
            let files = self
                .files_page(project, compatibility, page, budget)
                .await?;
            let done = files.len() < FILE_PAGE_SIZE;
            if let Some(file) = files
                .into_iter()
                .find(|file| file.id.to_string() == release)
            {
                if eligible(&file, project, compatibility) {
                    return normalize(file);
                }
                break;
            }
            if done {
                break;
            }
        }
        Err(ContentError::CandidateUnavailable(format!(
            "CurseForge release {release} could not be confirmed for {} on {}",
            compatibility.loader, compatibility.minecraft
        )))
    }

    async fn files_page(
        &self,
        project: &str,
        compatibility: &Compatibility,
        page: usize,
        budget: &mut super::RequestBudget,
    ) -> Result<Vec<dto::File>> {
        let url = files_url(project, compatibility, page)?;
        let response: dto::Response<Vec<dto::File>> = budget.get(self.request(url)?).await?;
        Ok(response.data)
    }
}

const MAX_FILE_PAGES: usize = 4;
const FILE_PAGE_SIZE: usize = 50;

fn files_url(project: &str, compatibility: &Compatibility, page: usize) -> Result<surf::Url> {
    let mut url = CurseForge::url(&["mods", project, "files"]);
    url.query_pairs_mut()
        .append_pair("gameVersion", &compatibility.minecraft)
        .append_pair("modLoaderType", loader_id(&compatibility.loader)?)
        .append_pair("pageSize", &FILE_PAGE_SIZE.to_string())
        .append_pair("index", &(page * FILE_PAGE_SIZE).to_string());
    Ok(url)
}

fn eligible(file: &dto::File, project: &str, compatibility: &Compatibility) -> bool {
    file.mod_id.to_string() == project
        && file.release_type == 1
        && file.is_server_pack != Some(true)
        && file.game_versions.contains(&compatibility.minecraft)
}

fn available_releases(versions: Vec<dto::File>) -> Result<Vec<CatalogRelease>> {
    let mut releases = Vec::new();
    for candidate in versions {
        match normalize(candidate) {
            Ok(release) => releases.push(release),
            Err(ContentError::CandidateUnavailable(reason)) => {
                tracing::debug!(%reason, "skipping unavailable CurseForge file")
            }
            Err(error) => return Err(error),
        }
        if releases.len() == 16 {
            break;
        }
    }
    Ok(releases)
}

fn loader_id(loader: &str) -> Result<&'static str> {
    match loader {
        "forge" => Ok("1"),
        "fabric" => Ok("4"),
        "quilt" => Ok("5"),
        "neoforge" => Ok("6"),
        _ => Err(ContentError::Invalid(format!(
            "CurseForge automatic selection for {loader} is not supported yet"
        ))),
    }
}

fn normalize(file: dto::File) -> Result<CatalogRelease> {
    let url = file
        .download_url
        .filter(|value| !value.is_empty())
        .filter(|_| file.is_available)
        .ok_or_else(|| {
            ContentError::CandidateUnavailable(format!(
                "{}: author or provider does not allow automatic installation",
                file.display_name
            ))
        })?;
    let download = DownloadFile {
        file_name: file.file_name,
        url,
        size_bytes: file.file_length,
        sha1: file
            .hashes
            .into_iter()
            .find(|hash| hash.algo == 1)
            .map(|hash| hash.value),
        sha512: None,
    };
    download.validate()?;
    Ok(CatalogRelease {
        key: ReleaseKey {
            project: project_key(Provider::CurseForge, file.mod_id.to_string()),
            release_id: file.id.to_string(),
        },
        title: file.display_name,
        dependencies: file
            .dependencies
            .into_iter()
            .map(|dependency| Dependency {
                project: project_key(Provider::CurseForge, dependency.mod_id.to_string()),
                exact_release: None,
                kind: mapping::dependency_kind(dependency.relation_type),
            })
            .collect(),
        file: download,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn compatible_file_listing_confirms_loader_and_skips_unavailable_releases() {
        let compatibility = Compatibility {
            minecraft: "1.21.1".into(),
            loader: "fabric".into(),
        };
        let url = files_url("2", &compatibility, 1).unwrap();
        assert_eq!(url.path(), "/v1/mods/2/files");
        let params: std::collections::HashMap<_, _> = url.query_pairs().collect();
        assert_eq!(params.get("modLoaderType").unwrap(), "4");
        assert_eq!(params.get("gameVersion").unwrap(), "1.21.1");
        assert_eq!(params.get("index").unwrap(), "50");

        let file = serde_json::json!({
            "id": 1, "modId": 2, "displayName": "Example", "fileName": "mod.jar",
            "releaseType": 1, "isAvailable": true, "fileDate": "2026-01-01",
            "fileLength": 3, "downloadUrl": "https://example.invalid/mod.jar",
            "gameVersions": ["1.21.1"], "hashes": [{"algo": 1, "value": "a".repeat(40)}]
        });
        let mut unavailable = file.clone();
        unavailable["id"] = serde_json::json!(3);
        unavailable["downloadUrl"] = serde_json::Value::Null;
        let versions =
            serde_json::from_value(serde_json::json!([unavailable, file.clone()])).unwrap();
        let releases = available_releases(versions).unwrap();
        assert_eq!(releases.len(), 1);
        assert_eq!(releases[0].key.release_id, "1");

        let mut server = file;
        server["isServerPack"] = serde_json::json!(true);
        assert!(!eligible(
            &serde_json::from_value(server).unwrap(),
            "2",
            &compatibility
        ));
    }

    #[test]
    fn unsupported_loader_does_not_silently_become_forge() {
        assert!(loader_id("cleanroom").is_err());
    }
    #[test]
    fn absent_key_is_an_explicit_error_without_network() {
        let provider = CurseForge::new(None);
        assert_eq!(
            CurseForge::url(&["mods", "search"]).path(),
            "/v1/mods/search"
        );
        assert!(matches!(
            provider.request(CurseForge::url(&["mods"])),
            Err(ContentError::MissingCurseForgeKey)
        ));
    }
    #[test]
    fn disabled_distribution_never_invents_a_cdn_url() {
        let file: dto::File = serde_json::from_value(serde_json::json!({
            "id":1,"modId":2,"displayName":"Mod","fileName":"mod.jar","releaseType":1,
            "isAvailable":true,"fileDate":"2026-01-01","fileLength":3,"downloadUrl":null,
            "gameVersions":["1.21.1"],"hashes":[],"dependencies":[]
        }))
        .unwrap();
        assert!(normalize(file).is_err());
    }
}
