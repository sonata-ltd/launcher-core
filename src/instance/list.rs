use async_std::stream::StreamExt;
use chrono::Utc;
use tide_websockets::WebSocketConnection;

use crate::{
    data::db::{Database, Result},
    websocket::messages::{
        operation::{
            event::OperationUpdate,
            process::{ProcessStatus, ProcessTarget},
            stage::OperationStage,
            OperationMessage,
        },
        scan::ScanInfo,
        BaseMessage, WsMessage, WsMessageType,
    },
};

const STAGE_TYPE: OperationStage = OperationStage::ScanInstances;

#[derive(Debug)]
struct Row {
    pub name: String,
    pub version: String,
    pub loader: String,
}

pub async fn get_instances(db: &Database, ws: &WebSocketConnection) -> Result<()> {
    let mut stream = sqlx::query_as!(
        Row,
        r#"
        SELECT COALESCE(o.changed_name, i.name) AS "name!", i.version, i.loader
        FROM instances i
        LEFT JOIN instances_overview o ON o.instance_id = i.id
        "#
    )
    .fetch(&db.pool);

    while let Some(row_res) = stream.next().await {
        let row = row_res?;

        let msg: WsMessage = <WsMessage<'_>>::from(OperationMessage {
            base: BaseMessage {
                message_id: "asd".to_string(),
                operation_id: Some("asd".to_string()),
                request_id: Some("asd".to_string()),
                timestamp: Utc::now(),
                correlation_id: None,
            },
            data: OperationUpdate::Determinable {
                stage: STAGE_TYPE,
                status: ProcessStatus::InProgress,
                target: Some(ProcessTarget::instance(
                    "".to_string(),
                    false,
                    "".to_string(),
                    false,
                    Some(ScanInfo {
                        name: row.name,
                        version: row.version,
                        loader: row.loader,
                    }),
                )),
                current: 0,
                total: 0,
            }
            .into(),
        });

        msg.send(&ws).await.unwrap();
    }

    Ok(())
}
