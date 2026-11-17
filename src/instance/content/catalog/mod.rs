//! Общий каталог выбирает адаптер. Wire-DTO и правила API находятся внутри адаптеров.
pub mod curseforge;
pub mod modrinth;
pub mod types;

use crate::instance::content::{
    error::{ContentError, Result},
    model::{ProjectKey, Provider},
};
use types::{CatalogRelease, Compatibility, SearchPage};

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
        match project.provider {
            Provider::Modrinth => {
                self.modrinth
                    .releases(&project.project_id, compatibility)
                    .await
            }
            Provider::CurseForge => {
                self.curseforge
                    .releases(&project.project_id, compatibility)
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
        match provider {
            Provider::Modrinth => self.modrinth.exact_release(release, compatibility).await,
            Provider::CurseForge => {
                self.curseforge
                    .exact_release(project, release, compatibility)
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
