use async_std::stream::StreamExt;
use chrono::Utc;
use tide_websockets::WebSocketConnection;

use crate::{
    data::db::{Database, Result},
    websocket::messages::{
        operation::stage::OperationStage,
        scan::{ScanData, ScanInfo, ScanIntegrity, ScanMessage},
        BaseMessage, WsMessage, WsMessageType,
    },
};


#[derive(Debug)]
struct Row {
    pub id: i64,
    pub name: String,
    pub version: String,
    pub loader: String,
}

pub async fn get_instances(db: &Database, ws: &WebSocketConnection) -> Result<()> {
    let mut stream = sqlx::query_as!(
        Row,
        r#"
        SELECT i.id, COALESCE(o.changed_name, i.name) AS "name!", i.version, i.loader
        FROM instances i
        LEFT JOIN instances_overview o ON o.instance_id = i.id
        "#
    )
    .fetch(&db.pool);

    while let Some(row_res) = stream.next().await {
        let row = row_res?;

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
                    id: row.id,
                    name: row.name,
                    version: row.version,
                    loader: row.loader,
                }),
            }
            .into(),
        });

        msg.send(&ws).await.unwrap();
    }

    Ok(())
}
