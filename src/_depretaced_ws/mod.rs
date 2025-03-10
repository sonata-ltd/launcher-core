use tide_websockets::WebSocketConnection;

pub mod messages;


pub async fn send_ws_msg(ws: &WebSocketConnection, msg: serde_json::Value) -> Result<(), String> {
    // let msg_in_json = serde_json::to_string(&msg).unwrap();

    ws.send_json(&msg).await.map_err(|e| {
        println!("WebSocket message sending error: {}", e);
        e.to_string()
    })
}
