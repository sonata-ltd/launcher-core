use chrono::{DateTime, Utc};
use operation::OperationMessage;
use scan::ScanMessage;
use serde::{Deserialize, Serialize};
use serde_json::json;
use thiserror::Error;
use tide_websockets::WebSocketConnection;
use ts_rs::TS;

pub mod operation;
pub mod scan;
pub mod task;

#[derive(Error, Debug)]
pub enum WsMessageError {
    #[error("Failed to send data though WebSocket: {0}")]
    SendFailed(String),
}

type Result<T> = std::result::Result<T, WsMessageError>;

pub trait WsMessageType: Serialize {
    async fn send(&self, ws: &WebSocketConnection) -> Result<()> {
        ws.send_json(&json!(&self))
            .await
            .map_err(|e| {
                println!("Failed to send WebSocket message, {}", e);
                return Err::<(), WsMessageError>(WsMessageError::SendFailed(e.to_string()));
            })
            .unwrap();

        return Ok(());
    }
}

impl WsMessageType for WsMessage {}

#[derive(Serialize, Deserialize, Debug)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
#[derive(TS)]
#[ts(export)]
pub enum WsMessage {
    Operation(OperationMessage),
    Scan(ScanMessage),
    Task,
}

#[derive(Serialize, Deserialize, Debug, Clone, TS)]
#[ts(export)]
pub struct BaseMessage {
    /// Unique ID of message
    pub message_id: String,

    /// ID of long-running operation
    #[serde(skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<String>,

    /// ID of source request from client
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,

    /// Time mark in RFC3339
    pub timestamp: DateTime<Utc>,

    /// ID of related operation (for chains)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<String>,
}
