//! Общий каталог выбирает адаптер. Wire-DTO и правила API находятся внутри адаптеров.
pub mod curseforge;
pub mod modrinth;
pub mod types;

use std::{collections::VecDeque, sync::Arc};

use crate::instance::content::{
    error::{ContentError, Result},
    model::{ProjectKey, Provider},
};
use types::{CatalogRelease, Compatibility, SearchPage};

const MAX_PLAN_REQUESTS: usize = 128;
const MAX_CACHED_MODRINTH_VERSIONS: usize = 8;

pub(crate) struct RequestBudget {
    remaining: usize,
    modrinth_versions: VecDeque<(String, Option<Arc<modrinth::VersionResponse>>)>,
}

impl Default for RequestBudget {
    fn default() -> Self {
        Self {
            remaining: MAX_PLAN_REQUESTS,
            modrinth_versions: VecDeque::new(),
        }
    }
}

impl RequestBudget {
    fn cached_modrinth_version(
        &mut self,
        id: &str,
    ) -> Option<Option<Arc<modrinth::VersionResponse>>> {
        let position = self
            .modrinth_versions
            .iter()
            .position(|(key, _)| key == id)?;
        let entry = self.modrinth_versions.remove(position)?;
        let response = entry.1.clone();
        self.modrinth_versions.push_back(entry);
        Some(response)
    }

    fn remember_modrinth_version(
        &mut self,
        id: String,
        response: Option<Arc<modrinth::VersionResponse>>,
    ) {
        if self.modrinth_versions.len() >= MAX_CACHED_MODRINTH_VERSIONS {
            self.modrinth_versions.pop_front();
        }
        self.modrinth_versions.push_back((id, response));
    }

    async fn get<T: serde::de::DeserializeOwned>(
        &mut self,
        request: surf::RequestBuilder,
    ) -> Result<T> {
        self.charge_request()?;
        Ok(crate::utils::download::json::get(request).await?)
    }

    pub(in crate::instance::content) fn charge_request(&mut self) -> Result<()> {
        self.remaining = self
            .remaining
            .checked_sub(1)
            .ok_or(ContentError::CatalogRequestLimit(MAX_PLAN_REQUESTS))?;
        Ok(())
    }
}

pub struct ContentCatalog {
    modrinth: modrinth::Modrinth,
    curseforge: curseforge::CurseForge,
}

impl ContentCatalog {
    pub fn new(curseforge_key: Option<String>) -> Self {
        Self {
            modrinth: modrinth::Modrinth::new(),
            curseforge: curseforge::CurseForge::new(curseforge_key),
        }
    }

    pub async fn search(
        &self,
        provider: Provider,
        query: &str,
        offset: u32,
        compatibility: &Compatibility,
    ) -> Result<SearchPage> {
        if query.len() > 512 {
            return Err(ContentError::Invalid(
                "search query exceeds the length limit".into(),
            ));
        }
        match provider {
            Provider::Modrinth => self.modrinth.search(query, offset, compatibility).await,
            Provider::CurseForge => self.curseforge.search(query, offset, compatibility).await,
        }
    }

    pub async fn releases(
        &self,
        project: &ProjectKey,
        compatibility: &Compatibility,
    ) -> Result<Vec<CatalogRelease>> {
        self.releases_bounded(project, compatibility, &mut RequestBudget::default())
            .await
    }

    pub(crate) async fn releases_bounded(
        &self,
        project: &ProjectKey,
        compatibility: &Compatibility,
        budget: &mut RequestBudget,
    ) -> Result<Vec<CatalogRelease>> {
        match project.provider {
            Provider::Modrinth => {
                self.modrinth
                    .releases_bounded(&project.project_id, compatibility, budget)
                    .await
            }
            Provider::CurseForge => {
                self.curseforge
                    .releases_bounded(&project.project_id, compatibility, budget)
                    .await
            }
        }
    }

    pub async fn exact_release(
        &self,
        provider: Provider,
        project: &str,
        release: &str,
        compatibility: &Compatibility,
    ) -> Result<CatalogRelease> {
        self.exact_release_bounded(
            provider,
            project,
            release,
            compatibility,
            &mut RequestBudget::default(),
        )
        .await
    }

    pub(crate) async fn exact_release_bounded(
        &self,
        provider: Provider,
        project: &str,
        release: &str,
        compatibility: &Compatibility,
        budget: &mut RequestBudget,
    ) -> Result<CatalogRelease> {
        match provider {
            Provider::Modrinth => {
                self.modrinth
                    .exact_release_bounded(release, compatibility, budget)
                    .await
            }
            Provider::CurseForge => {
                self.curseforge
                    .exact_release_bounded(project, release, compatibility, budget)
                    .await
            }
        }
    }
}

impl From<crate::utils::download::json::JsonRequestError> for ContentError {
    fn from(error: crate::utils::download::json::JsonRequestError) -> Self {
        match error {
            crate::utils::download::json::JsonRequestError::Status(status) => Self::Http(status),
            other => Self::Transport(other),
        }
    }
}

#[cfg(test)]
mod request_budget_tests {
    use super::*;

    #[async_std::test]
    async fn unavailable_version_lookup_reuses_cached_404_without_a_request() {
        let mut budget = RequestBudget {
            remaining: 0,
            ..Default::default()
        };
        budget.remember_modrinth_version("missing".into(), None);
        let catalog = ContentCatalog::new(None);
        let game = Compatibility {
            minecraft: "1.21.1".into(),
            loader: "fabric".into(),
        };
        assert!(matches!(
            catalog
                .exact_release_bounded(Provider::Modrinth, "project", "missing", &game, &mut budget)
                .await,
            Err(ContentError::CandidateUnavailable(_))
        ));
    }
}
