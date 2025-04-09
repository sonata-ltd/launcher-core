use std::sync::Arc;

use async_std::sync::Mutex;
use serde::{Deserialize, Serialize};
use ts_rs::TS;


pub type SharedTask<'a> = Arc<Mutex<Task<'a>>>;

#[derive(Serialize, Deserialize, Debug, TS)]
#[ts(export)]
pub struct Task<'a> {
    pub name: &'a str,
    // descr: Option<&'a str>,
    // is_active: Option<bool>,
    // progress_percentage: Option<bool>,
    // is_done: Option<bool>,
    // is_errored: Option<bool>,
}

pub struct TaskHandle<'a> {
    pub id: usize,
    pub task: SharedTask<'a>
}

impl<'a> Task<'a> {
    pub fn new_shared(name: &'a str) -> SharedTask<'a> {
        Arc::new(Mutex::new(Task { name }))
    }
}
