use async_std::stream::StreamExt;
use serde::Deserialize;
use serde_json::json;
use tide::StatusCode;
use tide_websockets::WebSocketConnection;
use tide_websockets::Message;
use uuid::Uuid;

use crate::instance::launch::ClientOptions;
use crate::instance::list::List;
use crate::instance::Instance;
use crate::EndpointRequest;


#[derive(Deserialize)]
struct CreateRequest {
    name: String,
    url: String,
    request_id: String,
}

pub async fn init_instance_ws<'a>(
    req: EndpointRequest<'a>,
    mut ws: WebSocketConnection,
) -> tide::Result<()> {
    while let Some(Ok(Message::Text(input))) = ws.next().await {
        let CreateRequest {
            name,
            url,
            request_id
        } = serde_json::from_str(&input).map_err(|e| {
            println!("Failed to parse JSON");
            tide::Error::from_str(400, format!("Failed to parse recieved JSON: {}", e))
        })?;

        let response: serde_json::Value;
        let instance = Instance::new(name, url, request_id);
        match Instance::init(instance, None, &req, &ws).await {
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

#[derive(Deserialize, Debug)]
struct RunRequest {
    name: String,
    url: String,
    request_id: String,
    launch_options: Option<ClientOptions>,
}

pub async fn run_instance_ws<'a>(
  req: EndpointRequest<'a>,
  mut ws: WebSocketConnection
) -> tide::Result<()> {
    while let Some(Ok(Message::Text(input))) = ws.next().await {
        let RunRequest {
            name,
            url,
            launch_options,
            request_id,
        } = serde_json::from_str(&input).map_err(|e| {
            println!("Failed to parse JSON");
            tide::Error::from_str(400, format!("Failed to parse recieved JSON: {}", e))
        })?;

        let response: serde_json::Value;
        let instance_request = Instance::new(
            name,
            url,
            request_id
        );

        match Instance::run(instance_request, launch_options, &req, &ws).await {
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

pub async fn list_instances_ws(mut ws: WebSocketConnection) -> tide::Result<()> {
    while let Some(Ok(Message::Text(_input))) = ws.next().await {
        println!("Updating instances list");

        let list_struct = List::new("/Users/quartix/.sonata/headers/main.json".to_string());

        let _result;
        match List::start_paths_checking(&list_struct, &ws).await {
            Ok(_) => {
                _result = json!({
                    "message": "Scan Completed"
                })
            }
            Err(e) => {
                _result = json!({
                    "message": e
                })
            }
        };

        // ws.send_string(format!("{result}")).await?;
    }

    Ok(())
}

pub async fn instance_options_dispatcher<'a>(req: EndpointRequest<'a>) -> tide::Result {
    let id_param = req.param("id")
        .map_err(|_| tide::Error::from_str(StatusCode::BadRequest, "Missing ID"))?;
    let _instance_id = Uuid::parse_str(id_param)
        .map_err(|_| tide::Error::from_str(StatusCode::BadRequest, "Invalid ID"))?;

    let _tab = req.param("tab")
        .map_err(|_| tide::Error::from_str(StatusCode::BadRequest, "Missing Tab"))?;

    Ok(tide::Response::builder(200)
        .body("ok")
        .content_type(tide::http::mime::PLAIN)
        .build())
}
