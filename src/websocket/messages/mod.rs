use chrono::{DateTime, Utc};
use operation::OperationMessage;
use scan::ScanMessage;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tide_websockets::WebSocketConnection;
use ts_rs::TS;

pub mod operation;
pub mod scan;
pub mod task;


pub trait WsMessageType: Serialize {
    async fn send(&self, ws: &WebSocketConnection) -> Result<(), surf::Error> {
        ws.send_json(&json!(&self)).await.map_err(|e| {
            println!("Failed to send WebSocket message, {}", e);
            return e;
        }).unwrap();

        return Ok(());
    }
}

impl<'a> WsMessageType for WsMessage<'a> {}


#[derive(Serialize, Deserialize, Debug)]
#[serde(
    tag = "type",
    content = "payload",
    rename_all = "snake_case"
)]
#[derive(TS)]
#[ts(export)]
pub enum WsMessage<'a> {
    #[serde(borrow)]
    Operation(OperationMessage<'a>),
    Scan(ScanMessage<'a>),
    Task
}

#[derive(Serialize, Deserialize, Debug)]
#[derive(TS)]
#[ts(export)]
pub struct BaseMessage<'a> {
    /// Unique ID of message
    pub message_id: &'a str,

    /// ID of long-running operation
    #[serde(skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<&'a str>,

    /// ID of source request from client
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<&'a str>,

    /// Time mark in RFC3339
    pub timestamp: DateTime<Utc>,

    /// ID of related operation (for chains)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<&'a str>
}
