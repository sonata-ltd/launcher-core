use std::sync::Arc;

use async_std::sync::Mutex;

use crate::websocket::messages::{
    operation::stage::OperationStage,
    task::{Task, TaskProgress, TaskStatus},
};

pub mod operations;

pub type SharedTask<'a> = Arc<Mutex<Task<'a>>>;

pub struct TaskHandle<'a> {
    pub id: usize,
    pub task: SharedTask<'a>,
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
