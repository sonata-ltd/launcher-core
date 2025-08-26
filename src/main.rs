use data::GlobalDataState;
use endpoints::{
    debug_ws, handle_init_root,
    instance::{init_instance_ws, instance_options_dispatcher, list_instances_ws, run_instance_ws},
    java::download_java_ws,
    versions::{get_version_ws, get_versions},
};

use http_types::headers::HeaderValue;
use serde_json::json;
use tide::security::CorsMiddleware;
use tide::{security::Origin, Request};
use tide_websockets::{Message, WebSocket, WebSocketConnection};

use crate::endpoints::versions::get_versions_unified;

pub mod instance;
pub mod java;
pub mod root;

// mod config;
mod data;
mod endpoints;
mod manifest;
mod utils;
mod websocket;

pub type EndpointRequest<'a> = Request<GlobalDataState<'a>>;

#[async_std::main]
async fn main() -> tide::Result<()> {
    let state = GlobalDataState::new().await;
    let mut app = tide::with_state(state);

    app.with(
        CorsMiddleware::new()
            .allow_origin(Origin::from("*"))
            .allow_methods("GET, POST".parse::<HeaderValue>().unwrap()),
    );

    // Init routes
    app.at("/init/root").post(handle_init_root);

    // Java routes
    app.at("/ws/java/install")
        .get(WebSocket::new(|_req, ws| download_java_ws(ws)));

    // Instance routes
    app.at("/instance/download_versions").post(get_versions);
    app.at("/instance/download_versions_unified")
        .get(get_versions_unified);
    app.at("/ws/instance/get_version")
        .get(WebSocket::new(|_req, ws| get_version_ws(ws)));

    app.at("/ws/instance/init")
        .get(WebSocket::new(|req, ws| init_instance_ws(req, ws)));
    app.at("/ws/instance/run")
        .get(WebSocket::new(|req, ws| run_instance_ws(req, ws)));
    app.at("/ws/instance/list")
        .get(WebSocket::new(|_req, ws| list_instances_ws(ws)));
    app.at("/instance/:id/:page")
        .get(instance_options_dispatcher);
    // app.at("/instance/options").get(instance_options_dispatcher);

    app.at("/debug/ws")
        .get(WebSocket::new(|_req, stream| debug_ws(stream)));
    app.at("/debug/tasks/notif")
        .get(WebSocket::new(|req, ws| debug_tasks(req, ws)));

    // Run server
    app.listen("127.0.0.1:8080").await?;

    Ok(())
}

async fn debug_tasks(req: EndpointRequest<'_>, ws: WebSocketConnection) -> tide::Result<()> {
    let all_tasks = req.state().get_all_tasks_json().await;
    if ws
        .send(Message::text(json!({"all_tasks": all_tasks}).to_string()))
        .await
        .is_err()
    {
        println!("Failed to send all tasks");
    }

    let mut rx = req.state().notifier.new_receiver();

    loop {
        match rx.recv().await {
            Ok(notif) => {
                if ws.send(Message::text(notif.to_string())).await.is_err() {
                    break;
                }
            }
            Err(e) => {
                eprintln!("Failed to receive notification: {:?}", e);
                break;
            }
        }
    }

    Ok(())
}
