use chrono::{DateTime, Utc};
use futures::{
    channel::oneshot,
    future::{BoxFuture, Shared},
    FutureExt,
};
use std::{
    collections::HashMap,
    panic::AssertUnwindSafe,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use super::{
    cache::{self, DiskIndex},
    error::{Result, VersionError},
    provider::{MetaProvider, VersionSummary},
    transport::{HttpIndexLoader, IndexLoader},
};

pub const INDEX_TTL: Duration = Duration::from_secs(6 * 60 * 60);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CatalogSource {
    Memory,
    Disk,
    Network,
}

#[derive(Debug, Clone)]
pub struct CatalogSnapshot {
    pub provider: MetaProvider,
    pub versions: Arc<Vec<VersionSummary>>,
    pub fetched_at: DateTime<Utc>,
    pub stale: bool,
    pub source: CatalogSource,
    pub warning: Option<String>,
}

type RefreshResult = std::result::Result<CatalogSnapshot, Arc<VersionError>>;
type Flight = Shared<BoxFuture<'static, RefreshResult>>;

struct Stored {
    snapshot: CatalogSnapshot,
    fresh_until: Instant,
}

struct Inner {
    default_provider: Mutex<MetaProvider>,
    cache: Mutex<HashMap<MetaProvider, Stored>>,
    flights: Mutex<HashMap<MetaProvider, Flight>>,
    cache_root: Option<PathBuf>,
    loader: Arc<dyn IndexLoader>,
}

pub struct VersionCatalog {
    inner: Arc<Inner>,
}

impl VersionCatalog {
    // Keep existing constructor for callers not requiring disk cache.
    pub fn new(default_provider: MetaProvider) -> Self {
        Self::build(default_provider, None, Arc::new(HttpIndexLoader))
    }

    pub fn with_cache(default_provider: MetaProvider, cache_root: PathBuf) -> Self {
        Self::build(
            default_provider,
            Some(cache_root),
            Arc::new(HttpIndexLoader),
        )
    }

    pub(super) fn build(
        default_provider: MetaProvider,
        cache_root: Option<PathBuf>,
        loader: Arc<dyn IndexLoader>,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                default_provider: Mutex::new(default_provider),
                cache: Mutex::new(HashMap::new()),
                flights: Mutex::new(HashMap::new()),
                cache_root,
                loader,
            }),
        }
    }

    pub async fn cached_snapshot(&self, provider: MetaProvider) -> Result<Option<CatalogSnapshot>> {
        provider.source()?;
        if let Some(snapshot) = self.memory(provider) {
            return Ok(Some(snapshot));
        }
        let Some(root) = self.inner.cache_root.clone() else {
            return Ok(None);
        };
        let Some(disk) = cache::read(root, provider).await? else {
            return Ok(None);
        };
        let versions = Arc::new(provider.source()?.parse_index(disk.raw.as_bytes())?);
        let now = Utc::now();
        let age = now.signed_duration_since(disk.fetched_at).to_std().ok();
        let remaining = age
            .and_then(|age| INDEX_TTL.checked_sub(age))
            .unwrap_or_default();
        let snapshot = CatalogSnapshot {
            provider,
            versions,
            fetched_at: disk.fetched_at,
            stale: remaining.is_zero(),
            source: CatalogSource::Disk,
            warning: None,
        };
        let mut cache = self.inner.cache.lock().unwrap();
        // A refresh may have finished while disk IO was running. Never
        // overwrite its newer snapshot with the older disk result.
        match cache.entry(provider) {
            std::collections::hash_map::Entry::Occupied(entry) => {
                Ok(Some(Self::copy_stored(entry.get(), CatalogSource::Memory)))
            }
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(Stored {
                    snapshot: snapshot.clone(),
                    fresh_until: Instant::now() + remaining,
                });
                Ok(Some(snapshot))
            }
        }
    }

    pub async fn refresh_from(&self, provider: MetaProvider) -> Result<CatalogSnapshot> {
        provider.source()?;
        let flight = {
            let mut flights = self.inner.flights.lock().unwrap();
            if let Some(flight) = flights.get(&provider) {
                flight.clone()
            } else {
                let (sender, receiver) = oneshot::channel::<RefreshResult>();
                let flight = async move {
                    receiver.await.unwrap_or_else(|_| {
                        Err(Arc::new(VersionError::IndexUnavailable(
                            "catalog task stopped".into(),
                        )))
                    })
                }
                .boxed()
                .shared();
                flights.insert(provider, flight.clone());
                let inner = Arc::clone(&self.inner);
                // Dropping the UI future drops only its receiver clone.
                async_std::task::spawn(async move {
                    let result = AssertUnwindSafe(Self::fetch(&inner, provider))
                        .catch_unwind()
                        .await
                        .unwrap_or_else(|_| {
                            Err(VersionError::IndexUnavailable(
                                "catalog task panicked".into(),
                            ))
                        })
                        .map_err(Arc::new);
                    // Hold the map lock through send/remove: a joining caller
                    // either shares this result or starts a genuinely new request.
                    let mut flights = inner.flights.lock().unwrap();
                    let _ = sender.send(result);
                    flights.remove(&provider);
                });
                flight
            }
        };
        flight.await.map_err(VersionError::Shared)
    }

    async fn fetch(inner: &Inner, provider: MetaProvider) -> Result<CatalogSnapshot> {
        let bytes = inner.loader.load(provider).await?;
        let versions = Arc::new(provider.source()?.parse_index(&bytes)?);
        let raw = String::from_utf8(bytes)
            .map_err(|error| VersionError::InvalidIndex(error.to_string()))?;
        let fetched_at = Utc::now();
        let mut snapshot = CatalogSnapshot {
            provider,
            versions,
            fetched_at,
            stale: false,
            source: CatalogSource::Network,
            warning: None,
        };
        // Validation completed before any attempt to replace the disk cache.
        if let Some(root) = inner.cache_root.clone() {
            if let Err(error) =
                cache::write(root, provider, DiskIndex::new(provider, fetched_at, raw)).await
            {
                tracing::warn!(%error, "could not persist version catalog");
                snapshot.warning = Some(error.to_string());
            }
        }
        inner.cache.lock().unwrap().insert(
            provider,
            Stored {
                snapshot: snapshot.clone(),
                fresh_until: Instant::now() + INDEX_TTL,
            },
        );
        Ok(snapshot)
    }

    fn copy_stored(stored: &Stored, source: CatalogSource) -> CatalogSnapshot {
        let mut snapshot = stored.snapshot.clone();
        snapshot.source = source;
        snapshot.stale = Instant::now() >= stored.fresh_until;
        snapshot
    }

    fn memory(&self, provider: MetaProvider) -> Option<CatalogSnapshot> {
        self.inner
            .cache
            .lock()
            .unwrap()
            .get(&provider)
            .map(|stored| Self::copy_stored(stored, CatalogSource::Memory))
    }

    pub async fn list_from(&self, provider: MetaProvider) -> Result<Arc<Vec<VersionSummary>>> {
        match self.cached_snapshot(provider).await {
            Ok(Some(snapshot)) if !snapshot.stale => return Ok(snapshot.versions),
            Err(error) => tracing::warn!(%error, "version cache unavailable; using network"),
            _ => {}
        }
        Ok(self.refresh_from(provider).await?.versions)
    }

    pub async fn resolve_from(&self, provider: MetaProvider, id: &str) -> Result<VersionSummary> {
        self.list_from(provider)
            .await?
            .iter()
            .find(|entry| entry.id == id)
            .cloned()
            .ok_or_else(|| VersionError::UnknownVersion {
                provider: provider.to_string(),
                id: id.into(),
            })
    }

    pub async fn list(&self) -> Result<Arc<Vec<VersionSummary>>> {
        self.list_from(self.default_provider()).await
    }

    pub async fn resolve(&self, id: &str) -> Result<VersionSummary> {
        self.resolve_from(self.default_provider(), id).await
    }

    pub fn set_default_provider(&self, provider: MetaProvider) {
        *self.inner.default_provider.lock().unwrap() = provider;
    }

    pub fn invalidate_all(&self) {
        self.inner.cache.lock().unwrap().clear();
    }

    fn default_provider(&self) -> MetaProvider {
        *self.inner.default_provider.lock().unwrap()
    }
}
