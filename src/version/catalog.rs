use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use crate::version::error::{Result, VersionError};
use crate::{
    utils::download::download,
    version::provider::{MetaProvider, VersionSummary},
};

pub struct VersionCatalog {
    default_provider: Mutex<MetaProvider>,
    cache: Mutex<HashMap<MetaProvider, CachedIndex>>,
}

struct CachedIndex {
    versions: Arc<Vec<VersionSummary>>,
    fetched_at: Instant,
}

const INDEX_TTL: Duration = Duration::from_secs(6 * 60 * 60);

impl VersionCatalog {
    pub fn new(default_provider: MetaProvider) -> Self {
        Self {
            default_provider: Mutex::new(default_provider),
            cache: Mutex::new(HashMap::new()),
        }
    }

    pub async fn list_from(&self, provider: MetaProvider) -> Result<Arc<Vec<VersionSummary>>> {
        if let Some(fresh) = self.cached(&provider) {
            return Ok(fresh);
        }

        let source = provider.source();

        let bytes = download(source.index_url().to_string())
            .await
            .map_err(|e| VersionError::IndexUnavailable(e.to_string()))?;

        let versions = Arc::new(source.parse_index(&bytes)?);

        self.store(provider, Arc::clone(&versions));
        Ok(versions)
    }

    pub async fn resolve_from(&self, provider: MetaProvider, id: &str) -> Result<VersionSummary> {
        self.list_from(provider)
            .await?
            .iter()
            .find(|v| v.id == id)
            .cloned()
            .ok_or_else(|| VersionError::UnknownVersion {
                provider: provider.to_string(),
                id: id.to_string(),
            })
    }

    pub async fn list(&self) -> Result<Arc<Vec<VersionSummary>>> {
        self.list_from(self.default_provider()).await
    }

    pub async fn resolve(&self, id: &str) -> Result<VersionSummary> {
        self.resolve_from(self.default_provider(), id).await
    }

    pub fn set_default_provider(&self, provider: MetaProvider) {
        *self.default_provider.lock().unwrap() = provider;
    }

    pub fn invalidate_all(&self) {
        let mut cache = self.cache.lock().unwrap();
        cache.clear();
    }

    fn cached(&self, provider: &MetaProvider) -> Option<Arc<Vec<VersionSummary>>> {
        let cache = self.cache.lock().unwrap();
        let entry = cache.get(&provider)?;

        (entry.fetched_at.elapsed() < INDEX_TTL).then(|| Arc::clone(&entry.versions))
    }

    fn store(&self, provider: MetaProvider, versions: Arc<Vec<VersionSummary>>) {
        self.cache.lock().unwrap().insert(
            provider,
            CachedIndex {
                versions,
                fetched_at: Instant::now(),
            },
        );
    }

    fn default_provider(&self) -> MetaProvider {
        *self.default_provider.lock().unwrap()
    }
}
