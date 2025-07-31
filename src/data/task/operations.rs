use serde_json::json;

use crate::data::{GlobalDataStateResult, GlobalAppDataError, GlobalDataState};

use super::*;

impl<'a> GlobalDataState<'a> {
    pub async fn update_task<F>(&self, task_id: usize, update: F) -> GlobalDataStateResult<()>
    where
        F: FnOnce(&mut Task),
    {
        let mut data = self.data.lock().await;

        if let Some(weak_task) = data.tasks_map.get(&task_id) {
            if let Some(task) = weak_task.upgrade() {
                let mut task = task.lock().await;
                update(&mut task);

                let msg = json!({
                    "task": {
                        "id": task_id,
                        "data": &*task
                    }
                });

                match self.notifier.broadcast(msg).await {
                    Ok(_) => return Ok(()),
                    Err(e) => return Err(GlobalAppDataError::BroadcastError(e.to_string())),
                }
            } else {
                data.tasks_map.remove(&task_id);
            }
        }

        Err(GlobalAppDataError::TaskNotFound(task_id))
    }

    pub async fn add_task(&self, task: Arc<Mutex<Task<'a>>>) -> GlobalDataStateResult<TaskHandle<'a>> {
        let mut data = self.data.lock().await;
        let task_id = data.tasks_map.len() + 1;
        data.tasks_map.insert(task_id, Arc::downgrade(&task));

        let current_task = {
            task.lock().await;
        };

        let payload = json!({
            "task": {
                "id": task_id,
                "data": current_task
            }
        });

        match self.notifier.broadcast(payload).await {
            Ok(_) => Ok(TaskHandle { id: task_id, task }),
            Err(e) => Err(GlobalAppDataError::BroadcastError(e.to_string())),
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
