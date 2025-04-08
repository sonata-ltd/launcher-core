use async_std::stream::StreamExt;
use serde_json::json;
use tide_websockets::WebSocketConnection;
use tide_websockets::Message;

use crate::root::LauncherRoot;
use crate::EndpointRequest;

pub mod java;
pub mod versions;
pub mod instance;


pub async fn debug_ws(mut stream: WebSocketConnection) -> tide::Result<()> {
    while let Some(Ok(Message::Text(input))) = stream.next().await {
        let output: String = input.chars().rev().collect();

        for _ in 0..10 {
            stream
                .send_string(format!("{} | {}", &input, &output))
                .await?;
        }
    }

    Ok(())
}

pub async fn handle_init_root<'a>(mut req: EndpointRequest<'a>) -> tide::Result {
    let launcher_root: LauncherRoot = req.body_json().await?;

    let response = json!({ "message": launcher_root.init_root() });

    Ok(tide::Response::builder(200)
        .body(response)
        .content_type(tide::http::mime::JSON)
        .build())
}
