use async_broadcast::broadcast;
use serde_json::json;

use crate::data::{GlobalDataStateResult, GlobalAppDataError, GlobalDataState};

use super::*;

impl<'a> GlobalDataState<'a> {
    pub fn create_task_broadcast() -> (Sender<serde_json::Value>, Receiver<serde_json::Value>) {
        let (mut tx, rx) = broadcast(16);
        tx.set_overflow(true);

        (tx, rx)
    }

    pub fn create_task_reciever(&self) -> Receiver<serde_json::Value> {
        self.data.tasks.notifier.new_receiver()
    }

    pub async fn update_task<F>(&self, task_id: usize, update: F) -> GlobalDataStateResult<()>
    where
        F: FnOnce(&mut Task),
    {
        let tasks = &self.data.tasks;
        let mut data = tasks.tasks_map.lock().await;

        if let Some(weak_task) = data.get(&task_id) {
            if let Some(task) = weak_task.upgrade() {
                let mut task = task.lock().await;
                update(&mut task);

                let msg = json!({
                    "task": {
                        "id": task_id,
                        "data": &*task
                    }
                });

                match tasks.notifier.broadcast(msg).await {
                    Ok(_) => return Ok(()),
                    Err(e) => return Err(GlobalAppDataError::BroadcastError(e.to_string())),
                }
            } else {
                data.remove(&task_id);
            }
        }

        Err(GlobalAppDataError::TaskNotFound(task_id))
    }

    pub async fn add_task(&self, task: Arc<Mutex<Task<'a>>>) -> GlobalDataStateResult<TaskData<'a>> {
        let tasks = &self.data.tasks;
        let mut data = tasks.tasks_map.lock().await;

        let task_id = data.len() + 1;
        data.insert(task_id, Arc::downgrade(&task));

        let current_task = {
            task.lock().await;
        };

        let payload = json!({
            "task": {
                "id": task_id,
                "data": current_task
            }
        });

        match tasks.notifier.broadcast(payload).await {
            Ok(_) => Ok(TaskData { id: task_id, task }),
            Err(e) => Err(GlobalAppDataError::BroadcastError(e.to_string())),
        }
    }

    pub async fn get_all_tasks_json(&self) -> Vec<serde_json::Value> {
        let tasks = &self.data.tasks;
        let data = tasks.tasks_map.lock().await;

        let mut result = Vec::new();

        for weak in data.values() {
            if let Some(task) = weak.upgrade() {
                let task = task.lock().await;
                result.push(json!(&*task));
            }
        }

        result
    }
}
