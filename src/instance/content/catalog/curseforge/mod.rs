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
        let mut versions = Vec::new();

        for page in 0..4 {
            let mut url = Self::url(&["mods", project, "files"]);
            url.query_pairs_mut()
                .append_pair("gameVersion", &compatibility.minecraft)
                .append_pair("modLoaderType", loader_id(&compatibility.loader)?)
                .append_pair("pageSize", "50")
                .append_pair("index", &(page * 50).to_string());
            let response: dto::Response<Vec<dto::File>> = json::get(self.request(url)?).await?;
            let done = response.data.len() < 50;
            versions.extend(response.data.into_iter().filter(|file| {
                file.release_type == 1 && file.game_versions.contains(&compatibility.minecraft)
            }));
            if done {
                break;
            }
        }
        versions.sort_by(|left, right| right.file_date.cmp(&left.file_date));
        versions.into_iter().take(16).map(normalize).collect()
    }

    pub async fn exact_release(
        &self,
        project: &str,
        release: &str,
        compatibility: &Compatibility,
    ) -> Result<CatalogRelease> {
        let response: dto::Response<dto::File> =
            json::get(self.request(Self::url(&["mods", project, "files", release]))?).await?;
        if !response
            .data
            .game_versions
            .contains(&compatibility.minecraft)
        {
            return Err(ContentError::Invalid(
                "CurseForge file version is not compatible with this Minecraft".into(),
            ));
        }
        normalize(response.data)
    }
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
            ContentError::Invalid(format!(
                "{}: author or provider does not allows automatic installation",
                file.display_name
            ))
        })?;
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
        file: DownloadFile {
            file_name: file.file_name,
            url,
            size_bytes: file.file_length,
            sha1: file
                .hashes
                .into_iter()
                .find(|hash| hash.algo == 1)
                .map(|hash| hash.value),
            sha512: None,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
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
