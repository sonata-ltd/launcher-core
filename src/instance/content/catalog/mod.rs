//! Общий каталог выбирает адаптер. Wire-DTO и правила API находятся внутри адаптеров.
pub mod curseforge;
pub mod modrinth;
pub mod types;

use crate::instance::content::{
    error::{ContentError, Result},
    model::{ProjectKey, Provider},
};
use types::{CatalogRelease, Compatibility, SearchPage};

const MAX_PLAN_REQUESTS: usize = 128;

pub(crate) struct RequestBudget {
    remaining: usize,
}

impl Default for RequestBudget {
    fn default() -> Self {
        Self {
            remaining: MAX_PLAN_REQUESTS,
        }
    }
}

impl RequestBudget {
    async fn get<T: serde::de::DeserializeOwned>(
        &mut self,
        request: surf::RequestBuilder,
    ) -> Result<T> {
        self.remaining = self
            .remaining
            .checked_sub(1)
            .ok_or(ContentError::CatalogRequestLimit(MAX_PLAN_REQUESTS))?;
        Ok(crate::utils::download::json::get(request).await?)
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
                "Слишком длинный поисковый запрос".into(),
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
            other => Self::Transport(other.to_string()),
        }
    }
}
