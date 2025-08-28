use std::{
    collections::HashMap,
    path::PathBuf,
    process::exit,
    sync::{Arc, Weak},
    usize,
};

use async_broadcast::{broadcast, Receiver, Sender};
use async_std::sync::{Mutex, MutexGuard};
use thiserror::Error;

use crate::{data::{config::Config, db::Database}, websocket::messages::task::Task};

mod config;
pub mod db;
pub mod definitions;
pub mod task;

pub type _GlobalAppDataGuard<'a> = MutexGuard<'a, GlobalAppData<'a>>;
pub type GlobalAppDataLocked<'a> = Arc<Mutex<GlobalAppData<'a>>>;

#[derive(Debug, Clone)]
pub struct GlobalAppData<'a> {
    pub tasks_map: HashMap<usize, Weak<Mutex<Task<'a>>>>,
}

#[derive(Debug, Clone)]
pub struct StaticData {
    pub launcher_root_path: PathBuf,
    pub db: Database,
}

#[derive(Debug, Clone)]
pub struct GlobalDataState<'a> {
    data: GlobalAppDataLocked<'a>,
    pub static_data: StaticData,
    pub notifier: Sender<serde_json::Value>,

    // Add receiver to structure to let websocket connection stay alive
    _receiver: Arc<Mutex<Receiver<serde_json::Value>>>,
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
        let (mut tx, rx) = broadcast(16);
        tx.set_overflow(true);

        let data = Arc::new(Mutex::new(GlobalAppData {
            tasks_map: HashMap::new(),
        }));

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
            },
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
            notifier: tx,
            _receiver: Arc::new(Mutex::new(rx)),
        }
    }

    pub async fn request_launcher_paths_migration() {
        println!("not implemented");
        // TODO: Implement
    }
}
