use std::{collections::HashMap, sync::{Arc, Weak}};

use async_broadcast::{Receiver, Sender};
use async_std::sync::Mutex;

use crate::websocket::messages::{
    operation::stage::OperationStage,
    task::{Task, TaskProgress, TaskStatus},
};

pub mod operations;

pub type SharedTask<'a> = Arc<Mutex<Task<'a>>>;
pub type TasksMap<'a> = HashMap<usize, Weak<Mutex<Task<'a>>>>;
pub type TasksMapLocked<'a> = Arc<Mutex<TasksMap<'a>>>;

#[derive(Debug)]
pub struct TaskData<'a> {
    pub id: usize,
    pub task: SharedTask<'a>,
}

#[derive(Debug, Clone)]
pub struct Tasks<'a> {
    pub tasks_map: TasksMapLocked<'a>,
    pub notifier: Sender<serde_json::Value>,

    // Add receiver to structure to let websocket connection stay alive
    pub _receiver: Arc<Mutex<Receiver<serde_json::Value>>>,
}

impl<'a> Task<'a> {
    pub fn new_shared(
        name: &'a str,
        status: TaskStatus,
        stage: Option<OperationStage>,
        progress: TaskProgress,
        message: Option<&'a str>,
    ) -> SharedTask<'a> {
        Arc::new(Mutex::new(Task {
            name,
            status,
            stage,
            progress,
            message,
        }))
    }
}
