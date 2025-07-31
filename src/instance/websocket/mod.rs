use std::sync::Arc;

use async_std::sync::Mutex;
use chrono::Utc;
use serde_json::json;
use surf::utils::async_trait;
use tide_websockets::WebSocketConnection;

use crate::websocket::messages::{
    operation::{
        event::{OperationEvent, OperationStart, OperationUpdate},
        process::{ProcessStatus, ProcessTarget},
        stage::{OperationStage, StageError, StageResult, StageStatus},
        OperationMessage,
    },
    BaseMessage, WsMessage,
};

// type Result<T> = std::result::Result<T, WsMessageError>;

#[derive(Debug, Clone)]
pub struct OperationWsMessage<'a> {
    msg: OperationMessage,
    ws: &'a WebSocketConnection,
}

impl<'a> From<(OperationMessage, &'a WebSocketConnection)> for OperationWsMessage<'a> {
    fn from((msg, ws): (OperationMessage, &'a WebSocketConnection)) -> Self {
        OperationWsMessage { msg, ws }
    }
}

impl<'a> From<OperationWsMessage<'a>> for WsMessage {
    fn from(wrapper: OperationWsMessage<'a>) -> WsMessage {
        WsMessage::Operation(wrapper.msg)
    }
}

pub type OperationWsMessageLocked<'a> = Arc<Mutex<OperationWsMessage<'a>>>;

impl<'a> OperationWsMessage<'a> {
    async fn send(&self) {
        let msg: WsMessage = WsMessage::Operation(self.msg.clone());

        self.ws
            .send_json(&json!(msg))
            .await
            .map_err(|e| {
                eprintln!("Failed to send WebSocket message, {}", e);

                // TODO: send error to special utility channel
                // return Err::<(), WsMessageError>(WsMessageError::SendFailed(e.to_string()));
            })
            .unwrap();
    }

    async fn update_and_send(arc: &Arc<Mutex<Self>>, data: impl Into<OperationEvent>) {
        let mut guard = arc.lock().await;
        guard.msg.data = data.into();
        guard.send().await;
    }

    pub async fn create_init_task(
        ws: &'a WebSocketConnection,
        request_id: &'a str,
    ) -> Arc<Mutex<Self>> {
        let stages = vec![
            OperationStage::FetchManifest,
            OperationStage::DownloadLibs,
            OperationStage::DownloadAssets,
        ];

        let op_msg = OperationMessage {
            base: BaseMessage {
                message_id: "asd".into(),
                operation_id: Some("todo".to_string()),
                request_id: Some(request_id.to_string()),
                timestamp: Utc::now(),
                correlation_id: None,
            },
            data: OperationStart { stages }.into(),
        };

        let wrapper: OperationWsMessage = (op_msg, ws).into();
        wrapper.send().await;
        Arc::new(Mutex::new(wrapper))
    }
}

#[async_trait]
pub trait OperationWsExt<'a> {
    async fn start_stage_determinable(
        self,
        stage: OperationStage,
        target: Option<ProcessTarget>,
        current: usize,
        total: usize,
    ) -> Self;

    async fn start_stage_indeterminable(self, stage: OperationStage) -> Self;

    async fn update_determinable(
        self,
        stage: OperationStage,
        target: Option<ProcessTarget>,
        current: usize,
        total: usize,
    ) -> Self;

    async fn complete_stage(
        self,
        status: StageStatus,
        stage: OperationStage,
        duration_secs: f64,
        error: Option<StageError>,
    ) -> Self;
}

#[async_trait]
impl<'a> OperationWsExt<'a> for Arc<Mutex<OperationWsMessage<'a>>> {
    async fn start_stage_determinable(
        self,
        stage: OperationStage,
        target: Option<ProcessTarget>,
        current: usize,
        total: usize,
    ) -> Self {
        OperationWsMessage::update_and_send(
            &self,
            OperationUpdate::Determinable {
                stage: stage.clone(),
                status: ProcessStatus::Started,
                target,
                current,
                total,
            },
        )
        .await;

        self
    }

    async fn start_stage_indeterminable(self, stage: OperationStage) -> Self {
        OperationWsMessage::update_and_send(
            &self,
            OperationUpdate::Indeterminable {
                stage,
                status: ProcessStatus::Started,
            },
        )
        .await;

        self
    }

    async fn update_determinable(
        self,
        stage: OperationStage,
        target: Option<ProcessTarget>,
        current: usize,
        total: usize,
    ) -> Self {
        OperationWsMessage::update_and_send(
            &self,
            OperationUpdate::Determinable {
                stage,
                status: ProcessStatus::InProgress,
                target,
                current,
                total,
            },
        )
        .await;

        self
    }

    async fn complete_stage(
        self,
        status: StageStatus,
        stage: OperationStage,
        duration_secs: f64,
        error: Option<StageError>,
    ) -> Self {
        OperationWsMessage::update_and_send(
            &self,
            OperationUpdate::Completed(StageResult {
                status,
                stage,
                duration_secs,
                error,
            }),
        )
        .await;

        self
    }
}
