use async_std::stream::StreamExt;
use chrono::Utc;
use http_types::mime::PLAIN;
use serde_json::json;
use tide::StatusCode;
use tide_websockets::Message;
use tide_websockets::WebSocketConnection;

use crate::instance::list::get_instances;
use crate::instance::options::pages::overview::OverviewFields;
use crate::instance::options::pages::settings::SettingsFields;
use crate::instance::options::pages::Page;
use crate::instance::options::ChangeRequestBuilder;
use crate::instance::InitData;
use crate::instance::Instance;
use crate::instance::RunData;
use crate::websocket::messages::option::InstanceFields;
use crate::websocket::messages::option::OptionUpdateMessage;
use crate::websocket::messages::BaseMessage;
use crate::websocket::messages::WsMessage;
use crate::websocket::messages::WsMessageType;
use crate::EndpointRequest;

pub async fn init_instance_ws<'a>(
    req: EndpointRequest<'a>,
    mut ws: WebSocketConnection,
) -> tide::Result<()> {
    while let Some(Ok(Message::Text(input))) = ws.next().await {
        let data: InitData = serde_json::from_str(&input).map_err(|e| {
            println!("Failed to parse JSON");
            tide::Error::from_str(400, format!("Failed to parse recieved JSON: {}", e))
        })?;

        let response: serde_json::Value;
        match Instance::init(data, None, &req, &ws).await {
            Ok(_) => {
                response = json!({
                    "message": "instance initialized"
                })
            }

            Err(e) => {
                println!("{e}");
                response = json!({
                    "result": format!("Failed"),
                    "error": format!("Failed to create instance, {}", e)
                });
            }
        }

        ws.send_string(format!("{response}")).await?;
    }

    Ok(())
}

pub async fn run_instance_ws<'a>(
    req: EndpointRequest<'a>,
    mut ws: WebSocketConnection,
) -> tide::Result<()> {
    while let Some(Ok(Message::Text(input))) = ws.next().await {
        let run_data: RunData = serde_json::from_str(&input).map_err(|e| {
            println!("Failed to parse JSON");
            tide::Error::from_str(400, format!("Failed to parse recieved JSON: {}", e))
        })?;

        let response: serde_json::Value;
        match Instance::run(run_data, &req, &ws).await {
            Ok(result) => response = json!(result),

            Err(e) => {
                println!("{e}");
                response = json!({
                    "result": format!("Failed"),
                    "error": format!("Failed to create instance, {}", e)
                });
            }
        }

        ws.send_string(format!("{response}")).await?;
    }

    Ok(())
}

pub async fn list_instances_ws<'a>(
    req: EndpointRequest<'a>,
    mut ws: WebSocketConnection,
) -> tide::Result<()> {
    while let Some(Ok(Message::Text(_input))) = ws.next().await {
        println!("Updating instances list");

        let db = &req.state().static_data.db;
        get_instances(&db, &ws).await.unwrap();

        ws.send_string(format!("done")).await?;
    }

    Ok(())
}

pub async fn instance_options_dispatcher<'a>(req: EndpointRequest<'a>) -> tide::Result {
    let id_param = req
        .param("id")
        .map_err(|_| tide::Error::from_str(StatusCode::BadRequest, "Missing ID"))?;
    let id = match id_param.parse::<i64>() {
        Ok(id) => id,
        Err(e) => {
            return Ok(tide::Response::builder(400)
                .body(e.to_string())
                .content_type(tide::http::mime::PLAIN)
                .build())
        }
    };

    let page_param = req
        .param("page")
        .map_err(|_| tide::Error::from_str(StatusCode::BadRequest, "Missing Tab"))?;
    let page: Page = page_param
        .parse()
        .map_err(|_| tide::Error::from_str(400, format!("Unknown page: {}", &page_param)))?;

    match Instance::get_page(&req, id, page).await {
        Ok(page_data) => {
            return Ok(tide::Response::builder(200)
                .body(page_data)
                .content_type(tide::http::mime::JSON)
                .build())
        }
        Err(e) => {
            return Ok(tide::Response::builder(500)
                .body(e.to_string())
                .content_type(tide::http::mime::PLAIN)
                .build())
        }
    }
}

pub async fn instance_option_change<'a>(mut req: EndpointRequest<'a>) -> tide::Result {
    let request: ChangeRequestBuilder = req.body_json().await?;
    if let Err(e) = Instance::change_field(&req, request).await {
        return Ok(tide::Response::builder(422)
            .body(e.to_string())
            .content_type(PLAIN)
            .build());
    }

    return Ok(tide::Response::builder(200).build());
}

pub async fn instance_options_sync(mut ws: WebSocketConnection) -> tide::Result<()> {
    while let Some(Ok(Message::Text(_))) = ws.next().await {
        let msg = WsMessage::Option(OptionUpdateMessage {
            base: BaseMessage {
                message_id: String::new(),
                operation_id: None,
                correlation_id: None,
                request_id: None,
                timestamp: Utc::now(),
            },
            option: InstanceFields::Overview(OverviewFields {
                name: Some("new name".into()),
                ..Default::default()
            }),
        });

        msg.send(&ws).await.unwrap();

        let msg = WsMessage::Option(OptionUpdateMessage {
            base: BaseMessage {
                message_id: String::new(),
                operation_id: None,
                correlation_id: None,
                request_id: None,
                timestamp: Utc::now(),
            },
            option: InstanceFields::Settings(SettingsFields {
                dir: Some("/Users/quartix/.sonata/instances/123".into())
            })
        });

        msg.send(&ws).await.unwrap();
    }

    Ok(())
}
