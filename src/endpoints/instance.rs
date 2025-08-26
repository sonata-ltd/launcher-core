use async_std::stream::StreamExt;
use serde_json::json;
use tide::StatusCode;
use tide_websockets::WebSocketConnection;
use tide_websockets::Message;

use crate::instance::list::List;
use crate::instance::options::pages::Page;
use crate::instance::InitData;
use crate::instance::Instance;
use crate::instance::RunData;
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
  mut ws: WebSocketConnection
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

pub async fn list_instances_ws(mut ws: WebSocketConnection) -> tide::Result<()> {
    while let Some(Ok(Message::Text(_input))) = ws.next().await {
        println!("Updating instances list");

        let list_struct = List::new("/Users/quartix/.sonata/headers/main.json".to_string());

        let _result;
        match List::start_paths_checking(&list_struct, &ws).await {
            Ok(_) => {
                println!("ok");
                _result = json!({
                    "message": "Scan Completed"
                })
            }
            Err(e) => {
                println!("{:#?}", e);
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
    let page_param = req.param("page")
        .map_err(|_| tide::Error::from_str(StatusCode::BadRequest, "Missing Tab"))?;
    let page: Page = page_param.parse().map_err(|_| {
        tide::Error::from_str(400, format!("Unknown page: {}", &page_param))
    })?;

    let global_app_state = req.state();
    let path_to_manifest = &global_app_state.static_data.launcher_root_path
        .join("headers")
        .join(id_param.to_owned() + ".json");

    match Instance::get_page(page, path_to_manifest).await {
        Ok(page_data) => {
            return Ok(tide::Response::builder(200)
                .body(page_data)
                .content_type(tide::http::mime::JSON)
                .build())
        },
        Err(e) => {
            return Ok(tide::Response::builder(500)
                .body(e.to_string())
                .content_type(tide::http::mime::PLAIN)
                .build())
        }
    }
}
