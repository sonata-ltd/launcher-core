use async_broadcast::broadcast;
use chrono::Utc;
use serde_json::json;

use crate::{
    data::{GlobalAppDataError, GlobalDataState, GlobalDataStateResult}, instance::list::get_instances, websocket::messages::{
        scan::{ScanData, ScanInfo, ScanIntegrity, ScanMessage},
        BaseMessage, WsMessage,
    }
};

use super::*;

impl<'a> GlobalDataState<'a> {
    pub fn create_instance_broadcast() -> (Sender<serde_json::Value>, Receiver<serde_json::Value>) {
        let (mut tx, rx) = broadcast(16);
        tx.set_overflow(true);

        (tx, rx)
    }
    pub fn create_instance_reciever(&self) -> Receiver<serde_json::Value> {
        self.data.tasks.notifier.new_receiver()
    }

    pub async fn init_instances_list(&self) {
        get_instances(&self.static_data.db, &self).await.unwrap();
    }

    pub async fn update_instance_name(
        &self,
        id: usize,
        new_name: Option<String>,
    ) -> GlobalDataStateResult<()> {
        let instances = &self.data.instances;
        let data = instances.instances_map.lock().await;

        if let Some(instance) = data.get(&id) {
            let mut instance = instance.lock().await;
            if instance.name != new_name {
                instance.name = new_name.clone();
                println!("updated");
            } else {
                return Ok(());
            }

            let new_name = match new_name {
                Some(name) => name,
                None => return Ok(())
            };

            // TODO: Implement handler
            let msg: WsMessage = <WsMessage<'_>>::from(ScanMessage {
                base: BaseMessage {
                    message_id: "asd".to_string(),
                    operation_id: Some("asd".to_string()),
                    request_id: Some("asd".to_string()),
                    timestamp: Utc::now(),
                    correlation_id: None,
                },
                data: ScanData {
                    integrity: ScanIntegrity {
                        instance_path: Some("".into()),
                    },
                    info: Some(ScanInfo {
                        id: instance.id,
                        name: new_name,
                        version: instance.version.clone(),
                        loader: instance.loader.clone(),
                    }),
                }
                .into(),
            });

            println!("sended");
            match instances.notifier.broadcast(json!(msg)).await {
                Ok(_) => return Ok(()),
                Err(e) => return Err(GlobalAppDataError::BroadcastError(e.to_string())),
            }
        }

        Err(GlobalAppDataError::TaskNotFound(id))
    }

    pub async fn add_instance(
        &self,
        instance: Arc<Mutex<InstanceDataRow>>,
    ) -> GlobalDataStateResult<()> {
        let instances = &self.data.instances;
        let mut data = instances.instances_map.lock().await;

        let instance_lock = instance.lock().await;
        data.insert(instance_lock.id as usize, instance.clone());

        let instance_name = match &instance_lock.name {
            Some(name) => name,
            None => {
                println!("name not found");
                return Ok(());
            }
        };

        let msg: WsMessage = <WsMessage<'_>>::from(ScanMessage {
            base: BaseMessage {
                message_id: "asd".to_string(),
                operation_id: Some("asd".to_string()),
                request_id: Some("asd".to_string()),
                timestamp: Utc::now(),
                correlation_id: None,
            },
            data: ScanData {
                integrity: ScanIntegrity {
                    instance_path: Some("".into()),
                },
                info: Some(ScanInfo {
                    id: instance_lock.id,
                    name: instance_name.to_owned(),
                    version: instance_lock.version.clone(),
                    loader: instance_lock.loader.clone(),
                }),
            }
            .into(),
        });

        match instances.notifier.broadcast(json!(msg)).await {
            Ok(_) => return Ok(()),
            Err(e) => return Err(GlobalAppDataError::BroadcastError(e.to_string())),
        }
    }

    pub async fn get_all_instances_json(&self) -> Vec<serde_json::Value> {
        let data = self.data.instances.instances_map.lock().await;
        let mut instances = Vec::new();

        for instance in data.values() {
            let instance = instance.lock().await;
            let name = match &instance.name {
                Some(name) => name,
                None => continue
            };


            let msg: WsMessage = <WsMessage<'_>>::from(ScanMessage {
                base: BaseMessage {
                    message_id: "asd".to_string(),
                    operation_id: Some("asd".to_string()),
                    request_id: Some("asd".to_string()),
                    timestamp: Utc::now(),
                    correlation_id: None,
                },
                data: ScanData {
                    integrity: ScanIntegrity {
                        instance_path: Some("".into()),
                    },
                    info: Some(ScanInfo {
                        id: instance.id,
                        name: name.clone(),
                        version: instance.version.clone(),
                        loader: instance.loader.clone(),
                    }),
                }
                .into(),
            });

            instances.push(json!(msg));
        }

        instances
    }
}
