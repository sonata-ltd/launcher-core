use std::sync::Arc;

use async_broadcast::{broadcast, Receiver, SendError, Sender};
use async_std::sync::{Mutex, MutexGuard};
use serde_json::json;
use task::Task;

pub mod task;


pub type _GlobalAppDataGuard<'a> = MutexGuard<'a, GlobalAppData<'a>>;
pub type GlobalAppDataLocked<'a> = Arc<Mutex<GlobalAppData<'a>>>;


#[derive(Debug)]
pub struct GlobalAppData<'a> {
    pub tasks: Vec<Task<'a>>,
}

#[derive(Debug, Clone)]
pub struct GlobalDataState<'a> {
    data: GlobalAppDataLocked<'a>,
    pub notifier: Sender<serde_json::Value>,

    // Add reciever to structure to let websocket connection stay alive
    _reciever: Arc<Mutex<Receiver<serde_json::Value>>>
}

impl<'a> GlobalDataState<'a> {
    pub fn new() -> Self {
        let (tx, rx) = broadcast(16);
        Self {
            data: Arc::new(Mutex::new(
                GlobalAppData { tasks: vec![] }
            )),
            notifier: tx,
            _reciever: Arc::new(Mutex::new(rx))
        }
    }

    pub async fn add_task(&self, task: Task<'a>) -> Result<(), String> {
        let mut data = self.data.lock().await;
        data.tasks.push(task);

        match self.notifier.broadcast(json!({"msg": data.tasks[data.tasks.len() - 1].name}).into()).await {
            Ok(_) => return Ok(()),
            Err(e) => return Err(e.to_string())
        };
    }
}
