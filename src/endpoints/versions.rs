use async_std::stream::StreamExt;
use serde::Deserialize;
use serde::Serialize;
use serde_json::json;
use serde_json::Value;
use tide_websockets::WebSocketConnection;
use tide_websockets::Message;

use crate::manifest::get_version_manifest;
use crate::EndpointRequest;


#[derive(Debug, Deserialize)]
struct Version<'a> {
    id: &'a str,
}


pub async fn get_versions<'a>(_req: EndpointRequest<'a>) -> tide::Result {
    let url = "https://piston-meta.mojang.com/mc/game/version_manifest_v2.json";

    let result;
    let code;

    match surf::get(url).await {
        Ok(mut response) => match response.body_json::<serde_json::Value>().await {
            Ok(data) => {
                result = data;
                code = 200;
            }
            Err(_) => {
                result = json!({ "message": "Failed to parse JSON" });
                code = 500;
            }
        },

        Err(_) => {
            result = json!({ "message": "Failed to download versions manifest" });
            code = 500;
        }
    }

    Ok(tide::Response::builder(code)
        .body(result)
        .content_type(tide::http::mime::JSON)
        .build())
}

pub async fn get_version_ws(mut ws: WebSocketConnection) -> tide::Result<()> {
    #[derive(Debug, Serialize)]
    struct Result {
        status: String,
        target: Value,
    }

    while let Some(Ok(Message::Text(input))) = ws.next().await {
        let version_request: Version = serde_json::from_str(&input).map_err(|e| {
            tide::Error::from_str(400, format!("Failed to parse recieved JSON: {}", e))
        })?;

        let result = match get_version_manifest(version_request.id).await {
            Ok(data) => {
                json!(Result {
                    status: "done".to_string(),
                    target: data
                })
            }
            Err(e) => json!({
                "error": e
            }),
        };

        println!("{}", result);
        ws.send_json(&result).await?;
    }

    Ok(())
}
