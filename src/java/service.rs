use std::{collections::HashSet, path::PathBuf, sync::Arc};

use async_std::sync::Mutex;

use crate::{
    bus::EventBus,
    data::{db::Database, state},
    java::{
        discovery::{self, DiscoveryContext},
        error::JavaResult,
        model::{JavaEvent, JavaRuntime, NewJavaRuntime},
        registry::JavaRegistry,
        storage::{DbJavaStore, JavaStore},
    },
};

pub const DEFAULT_JAVAREG_CACHE_SIZE: usize = 100;

pub struct JavaService {
    db: Arc<Database>,
    registry: Arc<JavaRegistry>,
    bus: EventBus,
    own_runtimes: PathBuf,
    /// Held for the whole search. Auto-selection waits on it too, so an
    /// instance installed during the first-run search sees its results.
    discovery: Mutex<()>,
}

impl JavaService {
    pub fn new(db: Arc<Database>, bus: EventBus, own_runtimes: PathBuf) -> Self {
        let store: Arc<dyn JavaStore> = Arc::new(DbJavaStore::new(Arc::clone(&db)));

        Self {
            db,
            registry: Arc::new(JavaRegistry::new(store, DEFAULT_JAVAREG_CACHE_SIZE)),
            bus,
            own_runtimes,
            discovery: Mutex::new(()),
        }
    }

    pub(crate) fn get_registry(&self) -> Arc<JavaRegistry> {
        Arc::clone(&self.registry)
    }

    pub async fn list_all(&self) -> JavaResult<Vec<Arc<JavaRuntime>>> {
        self.registry.list_all().await
    }

    pub async fn is_external_already_registerd(&self, exec_path: &PathBuf) -> JavaResult<bool> {
        Ok(self.registry.exists_by_exec_path(exec_path).await?)
    }

    pub async fn init_external(&self, new_java: NewJavaRuntime) -> JavaResult<Arc<JavaRuntime>> {
        let res = self.registry.register(new_java).await;

        match res {
            Ok(rec) => {
                self.bus.publish(JavaEvent::ListChanged);
                Ok(rec)
            }
            Err(e) => Err(e),
        }
    }

    /// The first-run search: does nothing once it has completed on this
    /// install. Returns how many runtimes it registered.
    pub async fn ensure_discovered(&self) -> JavaResult<usize> {
        let _guard = self.discovery.lock().await;

        self.registry.backfill_major().await?;

        if state::get_flag(&self.db, state::JAVA_DISCOVERY_DONE).await? {
            return Ok(0);
        }

        let added = self.discover_locked(false).await?;
        state::set_flag(&self.db, state::JAVA_DISCOVERY_DONE).await?;

        Ok(added)
    }

    /// Searches again whatever the flag says: the user asked for it.
    pub async fn rediscover(&self) -> JavaResult<usize> {
        let _guard = self.discovery.lock().await;
        self.discover_locked(true).await
    }

    /// Resolves once no search is running.
    pub async fn wait_discovery(&self) {
        drop(self.discovery.lock().await);
    }

    async fn discover_locked(&self, requested: bool) -> JavaResult<usize> {
        let cx = DiscoveryContext::system(self.own_runtimes.clone());
        let candidates = async_std::task::spawn_blocking(move || discovery::candidates(&cx)).await;

        let mut fresh = Vec::with_capacity(candidates.len());
        for path in candidates {
            if !self.registry.exists_by_exec_path(&path).await? {
                fresh.push(path);
            }
        }

        let found = discovery::probe_all(fresh).await;

        // A JDK the user added by another path (a symlink, `/usr/bin/java`)
        // is the same runtime: skip it by its home.
        let known_homes: HashSet<String> = self
            .registry
            .list_all()
            .await?
            .iter()
            .map(|j| j.home_path.clone())
            .collect();

        let new: Vec<NewJavaRuntime> = found
            .into_iter()
            .filter(|d| {
                d.home_path
                    .as_ref()
                    .is_none_or(|h| !known_homes.contains(h.to_string_lossy().as_ref()))
            })
            .map(NewJavaRuntime::from)
            .collect();

        let added = self.registry.register_many(new).await?.len();

        tracing::info!(added, requested, "java discovery finished");

        if added > 0 {
            self.bus.publish(JavaEvent::ListChanged);
        }
        self.bus
            .publish(JavaEvent::DiscoveryFinished { added, requested });

        Ok(added)
    }
}
