use std::{collections::HashMap, path::PathBuf, process::exit, sync::Arc, usize};

use async_std::sync::{Mutex, MutexGuard};
use thiserror::Error;

use crate::data::{config::Config, db::Database, instance::Instances, task::Tasks};

mod config;
pub mod db;
pub mod definitions;
mod instance;
mod task;

pub type _GlobalAppDataGuard<'a> = MutexGuard<'a, GlobalAppData<'a>>;

#[derive(Debug, Clone)]
pub struct GlobalAppData<'a> {
    pub tasks: Tasks<'a>,
    pub instances: Instances,
}

#[derive(Debug, Clone)]
pub struct StaticData {
    pub launcher_root_path: PathBuf,
    pub db: Database,
}

#[derive(Debug, Clone)]
pub struct GlobalDataState<'a> {
    pub data: GlobalAppData<'a>,
    pub static_data: StaticData,
}

#[derive(Error, Debug)]
pub enum GlobalAppDataError {
    #[error("Task with id {0} not found")]
    TaskNotFound(usize),

    #[error("Failed to broadcast message: {0}")]
    BroadcastError(String),

    #[error("Failed to get home path")]
    HomeDirNotFound,
}

pub type GlobalDataStateResult<T> = std::result::Result<T, GlobalAppDataError>;

impl<'a> GlobalDataState<'a> {
    pub async fn new() -> Self {
        let (task_tx, task_rx) = Self::create_task_broadcast();
        let (instances_tx, instances_rx) = Self::create_instance_broadcast();

        let data = GlobalAppData {
            tasks: Tasks {
                tasks_map: Arc::new(Mutex::new(HashMap::new())),
                notifier: task_tx,
                _receiver: Arc::new(Mutex::new(task_rx)),
            },
            instances: Instances {
                instances_map: Arc::new(Mutex::new(HashMap::new())),
                notifier: instances_tx,
                _reciever: Arc::new(Mutex::new(instances_rx)),
            },
        };

        let config = match Config::init().await {
            Ok(config) => config,
            Err(e) => {
                eprintln!("An unrecoverable error occured: {}", e.to_string());
                exit(1);
            }
        };

        let db = match Database::init(&config.get_db_path()).await {
            Ok(pool) => {
                println!("DB initialized");
                pool
            }
            Err(e) => {
                println!("Error occured on DB init: {e}");
                exit(1);
            }
        };

        Self {
            data,
            static_data: StaticData {
                launcher_root_path: config.take_launcher_root_path(),
                db,
            },
        }
    }

    pub async fn request_launcher_paths_migration() {
        println!("not implemented");
        // TODO: Implement
    }
}
