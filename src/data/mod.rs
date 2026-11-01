use std::{sync::Arc, time::Duration};

use async_std::task;
use thiserror::Error;

pub mod config;
pub mod db;
pub mod layout;
pub mod registry;
pub mod state;

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
    utils::download::sweep::sweep_partials,
    version::catalog::VersionCatalog,
};

pub struct LauncherServices {
    java: Arc<JavaService>,
    versions: Arc<VersionCatalog>,
    instances: Arc<InstanceService>,
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
    #[error("failed to initialize launcher paths: {0}")]
    Paths(#[from] LauncherPathsError),

    #[error("failed to initialize database: {0}")]
    Database(#[from] DbError),
}

pub type GlobalDataStateResult<T> = std::result::Result<T, AppError>;

impl GlobalState {
    pub async fn new(config: Config) -> Result<Self, AppError> {
        let paths = Arc::new(LauncherPaths::resolve(&config).await?);

        {
            let paths = Arc::clone(&paths);

            task::spawn(async move {
                const STALE_AFTER: Duration = Duration::from_secs(60 * 60);

                let removed = sweep_partials(paths.libraries(), STALE_AFTER).await
                    + sweep_partials(paths.assets(), STALE_AFTER).await;

                if removed > 0 {
                    tracing::info!(removed, "cleaned up partial downloads");
                }
            });
        }

        let db_url = config
            .database_url
            .clone()
            .unwrap_or_else(|| format!("sqlite://{}", paths.cache_db().display()));
        let db = Arc::new(Database::init(&db_url).await?);

        let bus = EventBus::new(DEFAULT_EVENTBUS_CAPACITY);
        let operations = Arc::new(OperationRegistry::new(bus.clone()));

        let java_service = Arc::new(JavaService::new(
            Arc::clone(&db),
            bus.clone(),
            paths.java_runtimes().clone(),
        ));
        let java_registry = java_service.get_registry();

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

        let services = LauncherServices {
            java: java_service,
            versions: Arc::new(VersionCatalog::new(config.meta_provider)),
            instances: instance_service,
            options: Arc::new(OptionsService::new(
                Arc::clone(&db),
                instance_registry,
                settings,
                java_registry,
                bus.clone(),
            )),
        };

        // The first-run Java search. In the background: `GlobalState::new`
        // is awaited before the GUI opens its window, and probing a hung
        // candidate takes seconds.
        {
            let java = Arc::clone(&services.java);
            let instances = Arc::clone(&services.instances);

            task::spawn(async move {
                match java.ensure_discovered().await {
                    Ok(added) if added > 0 => instances.assign_missing().await,
                    Ok(_) => {}
                    Err(e) => tracing::warn!(error = %e, "java discovery failed"),
                }
            });
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

    pub fn java(&self) -> Arc<JavaService> {
        Arc::clone(&self.services.java)
    }

    pub fn versions(&self) -> Arc<VersionCatalog> {
        Arc::clone(&self.services.versions)
    }

    pub fn instances(&self) -> Arc<InstanceService> {
        Arc::clone(&self.services.instances)
    }

    pub fn options(&self) -> Arc<OptionsService> {
        Arc::clone(&self.services.options)
    }
}
