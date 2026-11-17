use crate::instance::content::model::{Candidate, ProjectKey, Provider, ReleaseKey};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogProject {
    pub key: ProjectKey,
    pub title: String,
    pub summary: String,
    pub author: String,
    pub downloads: u64,
}

#[derive(Debug, Clone)]
pub struct SearchPage {
    pub projects: Vec<CatalogProject>,
    pub offset: u32,
    pub total: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadFile {
    pub file_name: String,
    pub url: String,
    pub size_bytes: u64,
    pub sha1: Option<String>,
    pub sha512: Option<String>,
}

impl DownloadFile {
    /// Наличие хотя бы одного ожидаемого hash обязательно. Проверяются все
    /// предоставленные алгоритмы, а не только первый подходящий.
    pub fn matches(&self, actual: &crate::utils::files::digest::FileDigests) -> bool {
        self.size_bytes == actual.size_bytes
            && (self.sha1.is_some() || self.sha512.is_some())
            && self
                .sha1
                .as_ref()
                .is_none_or(|expected| expected.eq_ignore_ascii_case(&actual.sha1))
            && self
                .sha512
                .as_ref()
                .is_none_or(|expected| expected.eq_ignore_ascii_case(&actual.sha512))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn all_available_hashes_and_size_are_checked() {
        let actual = crate::utils::files::digest::FileDigests {
            sha256: "local".into(),
            sha1: "abc".into(),
            sha512: "def".into(),
            size_bytes: 3,
        };
        let mut file = DownloadFile {
            file_name: "mod.jar".into(),
            url: "https://example.invalid/mod.jar".into(),
            size_bytes: 3,
            sha1: None,
            sha512: Some("DEF".into()),
        };
        assert!(file.matches(&actual));
        file.sha1 = Some("bad".into());
        assert!(!file.matches(&actual));
        file.sha1 = Some("ABC".into());
        assert!(file.matches(&actual));
        file.size_bytes = 4;
        assert!(!file.matches(&actual));
        file.size_bytes = 3;
        file.sha1 = None;
        file.sha512 = None;
        assert!(!file.matches(&actual));
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogRelease {
    pub key: ReleaseKey,
    pub title: String,
    pub file: DownloadFile,
    pub dependencies: Vec<crate::instance::content::model::Dependency>,
}

impl CatalogRelease {
    pub fn candidate(&self) -> Candidate {
        Candidate {
            key: self.key.clone(),
            dependencies: self.dependencies.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Compatibility {
    pub minecraft: String,
    pub loader: String,
}

pub fn project_key(provider: Provider, project_id: impl Into<String>) -> ProjectKey {
    ProjectKey {
        provider,
        project_id: project_id.into(),
    }
}
