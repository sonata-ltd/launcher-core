use std::{collections::HashMap, sync::Arc};

use async_broadcast::{Receiver, Sender};
use async_std::sync::Mutex;

use crate::instance::list::InstanceDataRow;

pub type SharedInstanceRow = Arc<Mutex<InstanceDataRow>>;
pub type InstancesMap = HashMap<usize, Arc<Mutex<InstanceDataRow>>>;
pub type InstancesMapLocked = Arc<Mutex<InstancesMap>>;

pub mod operations;


#[derive(Debug, Clone)]
pub struct Instances {
    pub instances_map: InstancesMapLocked,
    pub notifier: Sender<serde_json::Value>,

    // Add receiver to structure to let WebSocket connection stay alive
    pub _reciever: Arc<Mutex<Receiver<serde_json::Value>>>
}

impl InstanceDataRow {
    pub fn new_shared(
        id: i64,
        name: Option<String>,
        version: String,
        loader: String,
    ) -> SharedInstanceRow {
        Arc::new(Mutex::new(InstanceDataRow {
            id,
            name,
            version,
            loader,
        }))
    }
}
