use std::{
    collections::HashMap,
    sync::{Arc, Weak},
    usize,
};

use async_broadcast::{broadcast, Receiver, Sender};
use async_std::sync::{Mutex, MutexGuard};
use serde_json::json;
use task::{SharedTask, Task, TaskHandle};
use thiserror::Error;

pub mod task;


pub type _GlobalAppDataGuard<'a> = MutexGuard<'a, GlobalAppData<'a>>;
pub type GlobalAppDataLocked<'a> = Arc<Mutex<GlobalAppData<'a>>>;
pub type TasksMap<'a> = HashMap<usize, Weak<Mutex<Task<'a>>>>;

#[derive(Debug)]
pub struct GlobalAppData<'a> {
    pub tasks_map: HashMap<usize, Weak<Mutex<Task<'a>>>>,
}

#[derive(Debug, Clone)]
pub struct GlobalDataState<'a> {
    data: GlobalAppDataLocked<'a>,
    pub notifier: Sender<serde_json::Value>,

    // Add reciever to structure to let websocket connection stay alive
    _reciever: Arc<Mutex<Receiver<serde_json::Value>>>,
}

#[derive(Error, Debug)]
pub enum GlobalAppDataError {
    #[error("Task with id {0} not found")]
    TaskNotFound(usize),

    #[error("Failed to broadcast message: {0}")]
    BroadcastError(String)
}

pub type Result<T> = std::result::Result<T, GlobalAppDataError>;

impl<'a> GlobalDataState<'a> {
    pub fn new() -> Self {
        let (mut tx, rx) = broadcast(16);
        tx.set_overflow(true);

        let data = Arc::new(Mutex::new(GlobalAppData {
            tasks_map: HashMap::new(),
        }));

        Self {
            data,
            notifier: tx,
            _reciever: Arc::new(Mutex::new(rx)),
        }
    }

    pub async fn update_task<F>(&self, task_id: usize, update: F) -> Result<()>
    where
        F: FnOnce(&mut Task),
    {
        let mut data = self.data.lock().await;

        if let Some(weak_task) = data.tasks_map.get(&task_id) {
            if let Some(task) = weak_task.upgrade() {
                let mut task = task.lock().await;
                update(&mut task);

                match self.notifier.broadcast(json!(&*task)).await {
                    Ok(_) => return Ok(()),
                    Err(e) => return Err(GlobalAppDataError::BroadcastError(e.to_string()))
                }
            } else {
                data.tasks_map.remove(&task_id);
            }
        }

        Err(GlobalAppDataError::TaskNotFound(task_id))
    }

    pub async fn add_task(&self, task: Arc<Mutex<Task<'a>>>) -> Result<TaskHandle<'a>> {
        let mut data = self.data.lock().await;
        let task_id = data.tasks_map.len() + 1;
        data.tasks_map.insert(task_id, Arc::downgrade(&task));

        let task_name = {
            let t = task.lock().await;
            t.name
        };

        match self.notifier.broadcast(json!({"msg": task_name}).into()).await {
            Ok(_) => Ok(TaskHandle { id: task_id, task }),
            Err(e) => Err(GlobalAppDataError::BroadcastError(e.to_string()))
        }
    }

    pub async fn get_all_tasks_json(&self) -> Vec<serde_json::Value> {
        let data = self.data.lock().await;

        let mut result = Vec::new();

        for weak in data.tasks_map.values() {
            if let Some(task) = weak.upgrade() {
                let task = task.lock().await;
                result.push(json!(&*task));
            }
        }

        result
    }
}
