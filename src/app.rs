use std::{sync::Arc, time::Duration};

use async_std::task;
use thiserror::Error;

use crate::{
    bus::EventBus,
    data::{
        config::{Config, DEFAULT_EVENTBUS_CAPACITY},
        db::{Database, DbError},
        layout::{LauncherPaths, LauncherPathsError},
        registry::operation::OperationRegistry,
    },
    instance::{
        options::service::OptionsService, service::InstanceService,
        settings::service::SettingsService,
    },
    java::{
        error::JavaResult,
        model::{JavaRuntime, NewJavaRuntime},
        service::JavaService,
    },
    modloader::catalog::ModloaderCatalog,
    utils::download::sweep::sweep_partials,
    version::catalog::VersionCatalog,
};

pub struct LauncherServices {
    content: Arc<crate::instance::content::service::InstanceContentService>,
    java: Arc<JavaService>,
    versions: Arc<VersionCatalog>,
    instances: Arc<InstanceService>,
    modloaders: Arc<crate::modloader::catalog::ModloaderCatalog>,
    options: Arc<OptionsService>,
}

pub struct GlobalState {
    pub db: Arc<Database>,
    pub bus: EventBus,
    pub config: Config,
    pub paths: Arc<LauncherPaths>,
    pub operations: Arc<OperationRegistry>,
    pub services: LauncherServices,
}

#[derive(Error, Debug)]
pub enum AppError {
    #[error("failed to initialize runtime retention: {0}")]
    RuntimeRetention(#[from] crate::instance::init::error::InitError),
    #[error(transparent)]
    Content(#[from] crate::instance::content::error::ContentError),

    #[error("failed to initialize launcher paths: {0}")]
    Paths(#[from] LauncherPathsError),

    #[error("failed to initialize database: {0}")]
    Database(#[from] DbError),
}

pub type GlobalDataStateResult<T> = std::result::Result<T, AppError>;

impl GlobalState {
    pub fn content(&self) -> Arc<crate::instance::content::service::InstanceContentService> {
        self.services.content.clone()
    }

    pub async fn new(config: Config) -> Result<Self, AppError> {
        let paths = Arc::new(LauncherPaths::resolve(&config).await?);

        let db_url = config
            .database_url
            .clone()
            .unwrap_or_else(|| format!("sqlite://{}", paths.cache_db().display()));
        let db = Arc::new(Database::init(&db_url).await?);
        crate::instance::init::retention::initialize(db.clone(), paths.clone()).await?;
        crate::instance::download::assets::AssetsData::initialize(
            db.clone(),
            paths.assets().clone(),
        )
        .await?;

        let bus = EventBus::new(DEFAULT_EVENTBUS_CAPACITY);
        let operations = Arc::new(OperationRegistry::new(bus.clone()));

        let java_service = Arc::new(JavaService::with_operations(
            Arc::clone(&db),
            bus.clone(),
            paths.java_runtimes().clone(),
            operations.clone(),
        ));
        let java_registry = java_service.get_registry();
        if let Err(error) = java_service.initialize().await {
            tracing::debug!(%error, "managed Java SQL path upgrade deferred");
        }

        let settings = Arc::new(SettingsService::new(Arc::clone(&db)));

        let instance_service = Arc::new(InstanceService::new(
            Arc::clone(&db),
            Arc::clone(&settings),
            Arc::clone(&java_service),
            Arc::clone(&operations),
            Arc::clone(&paths),
            bus.clone(),
        ));

        let instance_registry = instance_service.get_registry();

        let content_service = Arc::new(
            crate::instance::content::service::InstanceContentService::new(
                db.clone(),
                paths.clone(),
                instance_service.clone(),
                config.curseforge_api_key.clone(),
                operations.clone(),
            )
            .await?,
        );

        let services = LauncherServices {
            content: content_service,
            java: java_service,
            versions: Arc::new(VersionCatalog::with_cache(
                config.meta_provider,
                paths.meta().join("version-catalog"),
            )),
            instances: instance_service,
            modloaders: Arc::new(crate::modloader::catalog::ModloaderCatalog::new(
                paths.meta().join("modloader-catalog"),
            )),
            options: Arc::new(OptionsService::new(
                Arc::clone(&db),
                instance_registry,
                settings,
                java_registry,
                bus.clone(),
            )),
        };

        // Background workers start only after every fallible bootstrap step.
        {
            let paths = paths.clone();
            operations
                .tasks
                .spawn("startup scratch maintenance", None, async move {
                    const STALE_AFTER: Duration = Duration::from_secs(60 * 60);
                    let removed = sweep_partials(paths.libraries(), STALE_AFTER).await
                        + sweep_partials(paths.assets(), STALE_AFTER).await;
                    tracing::debug!(removed, "partial download maintenance complete");
                    crate::instance::launch::natives::sweep_stale(paths.instances().clone()).await;
                    let blobs = paths.libraries().join("blobs");
                    task::spawn_blocking(move || {
                        let store = crate::utils::object_store::ObjectStore::new(blobs);
                        if let Err(error) = store.sweep_staging() {
                            tracing::warn!(%error, "runtime scratch maintenance deferred");
                        }
                    })
                    .await;
                })
                .expect("bootstrap task admission open");
        }

        // The first-run Java search. In the background: `GlobalState::new`
        // is awaited before the GUI opens its window, and probing a hung
        // candidate takes seconds.
        {
            let java = Arc::clone(&services.java);
            let instances = Arc::clone(&services.instances);

            operations
                .tasks
                .spawn("startup Java discovery", None, async move {
                    match java.ensure_discovered().await {
                        Ok(added) if added > 0 => instances.assign_missing().await,
                        Ok(_) => {}
                        Err(e) => tracing::warn!(error = %e, "java discovery failed"),
                    }
                })
                .expect("bootstrap task admission open");
        }

        Ok(Self {
            db,
            bus,
            config,
            paths,
            operations,
            services,
        })
    }

    /// Registers a runtime the user picked by hand, then gives it to every
    /// instance that has no Java yet and can use this one.
    pub async fn add_java(&self, new: NewJavaRuntime) -> JavaResult<Arc<JavaRuntime>> {
        let java = self.services.java.init_external(new).await?;
        self.services.instances.assign_missing().await;

        Ok(java)
    }

    /// Searches for installed runtimes again, regardless of the first-run
    /// flag, then retries every instance without a Java — also when nothing
    /// new turned up: one may fit a runtime registered before it was installed.
    pub async fn rediscover_java(&self) -> JavaResult<usize> {
        let added = self.services.java.rediscover().await?;
        self.services.instances.assign_missing().await;

        Ok(added)
    }

    pub async fn validate_managed_java(
        self: &Arc<Self>,
        id: uuid::Uuid,
    ) -> crate::java::managed::Result<Arc<JavaRuntime>> {
        let core = Arc::clone(self);
        let (tx, rx) = async_channel::bounded(1);
        self.operations
            .tasks
            .spawn("managed Java assignment", None, async move {
                let java = core.java();
                let result = java.validate_managed(id).await;
                if result.is_ok() {
                    core.services.instances.assign_missing().await;
                }
                let _ = tx.send(result).await;
            })
            .map_err(|error| crate::java::managed::ManagedJavaError::new("shutdown", error))?;
        rx.recv()
            .await
            .map_err(|e| crate::java::managed::ManagedJavaError::new("validation", e))?
    }

    pub fn java(&self) -> Arc<JavaService> {
        Arc::clone(&self.services.java)
    }

    /// Seal worker admission and await cooperative cancellation, commits and
    /// cleanup. Running games are not killed implicitly: stop them explicitly
    /// or await their exit. This does not close shared DB pools underneath users.
    pub async fn shutdown(&self) -> Vec<crate::utils::jobs::supervisor::TaskResult> {
        self.operations.tasks.shutdown().await
    }

    pub fn versions(&self) -> Arc<VersionCatalog> {
        Arc::clone(&self.services.versions)
    }

    pub fn instances(&self) -> Arc<InstanceService> {
        Arc::clone(&self.services.instances)
    }

    pub fn modloaders(&self) -> Arc<ModloaderCatalog> {
        Arc::clone(&self.services.modloaders)
    }

    pub fn options(&self) -> Arc<OptionsService> {
        Arc::clone(&self.services.options)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[async_std::test]
    async fn failed_database_bootstrap_does_not_start_background_maintenance() {
        let root = tempfile::tempdir().unwrap();
        let sentinel = root.path().join("libraries/unknown.part");
        std::fs::create_dir_all(sentinel.parent().unwrap()).unwrap();
        std::fs::write(&sentinel, b"user bytes").unwrap();
        let config = Config {
            root_override: Some(root.path().into()),
            database_url: Some(format!("sqlite://{}", root.path().display())),
            ..Config::init()
        };
        assert!(matches!(
            GlobalState::new(config).await,
            Err(AppError::Database(_))
        ));
        assert_eq!(std::fs::read(sentinel).unwrap(), b"user bytes");
    }

    #[test]
    fn data_compatibility_alias_has_the_same_application_type_without_owning_composition() {
        let application: Option<GlobalState> = None;
        let _: Option<crate::data::GlobalState> = application;
        assert!(!include_str!("data/mod.rs").contains("task::spawn"));
    }
}
