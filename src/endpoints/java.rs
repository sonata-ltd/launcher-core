use async_std::stream::StreamExt;
use home::home_dir;
use serde::Deserialize;
use tide_websockets::WebSocketConnection;
use tide_websockets::Message;

use crate::java::Java;


#[derive(Debug, Deserialize)]
struct DownloadRequest {
    java_ver: String,
}

pub async fn download_java_ws(mut ws: WebSocketConnection) -> tide::Result<()> {
    while let Some(Ok(Message::Text(input))) = ws.next().await {
        let download_request: DownloadRequest = serde_json::from_str(&input).map_err(|e| {
            tide::Error::from_str(400, format!("Failed to parse recieved JSON: {}", e))
        })?;

        let DownloadRequest { java_ver } = download_request;
        // let available_java_url = "https://launchermeta.mojang.com/v1/products/java-runtime/2ec0cc96c44e5a76b9c8b7c39df7210883d12871/all.json";

        match home_dir() {
            Some(path) => {
                let java_path = format!("{}/.sonata/java", path.display());
                let metacache_path = format!("{}/.sonata/metacache.json", path.display());
                let java_properties = Java::new(
                    "21".to_string(),
                    "java-runtime-delta".to_string(),
                    java_path,
                );
                Java::init(java_properties, metacache_path).await.unwrap();
            }
            None => (),
        };

        println!("Recieved java version: {}", java_ver);
    }

    Ok(())
}
