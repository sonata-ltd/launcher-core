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

use crate::{utils::get_home_dir, websocket::messages::task::Task};

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

        let launcher_root_path = match get_home_dir().await {
            Some(path) => path.join(".sonata"),
            None => exit(1),
        };

        Self {
            data,
            static_data: StaticData { launcher_root_path },
            notifier: tx,
            _receiver: Arc::new(Mutex::new(rx)),
        }
    }

    pub async fn request_launcher_paths_migration() {
        println!("not implemented");
        // TODO: Implement
    }
}
