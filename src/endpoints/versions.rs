use async_std::stream::StreamExt;
use serde::Deserialize;
use serde::Serialize;
use serde_json::json;
use serde_json::Value;
use tide_websockets::Message;
use tide_websockets::WebSocketConnection;

use crate::manifest::get_version_manifest;
use crate::utils::unify::UnifiedVersionsData;
use crate::EndpointRequest;

#[derive(Debug, Deserialize)]
struct Version<'a> {
    id: &'a str,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
enum ManifestTypes {
    MojangVersionManifest,
    PrismVersionManifest,
}

#[derive(Deserialize)]
struct ManifestRequest {
    manifest_type: ManifestTypes,
}

pub async fn get_versions_unified<'a>(mut _req: EndpointRequest<'a>) -> tide::Result {
    let unified_versions =
        match UnifiedVersionsData::new(crate::utils::unify::MetaProviders::Prism).await {
            Ok(manifest) => manifest,
            Err(e) => return Ok(tide::Response::builder(e).build()),
        };

    match unified_versions.build() {
        Ok(data) => Ok(tide::Response::builder(200)
            .body(data)
            .content_type(tide::http::mime::JSON)
            .build()),
        Err(e) => Ok(tide::Response::builder(422)
            .body(e)
            .content_type(tide::http::mime::PLAIN)
            .build()),
    }
}

pub async fn get_versions<'a>(mut req: EndpointRequest<'a>) -> tide::Result {
    let ManifestRequest { manifest_type } = req.body_json().await?;
    let url = match manifest_type {
        ManifestTypes::MojangVersionManifest => {
            "https://piston-meta.mojang.com/mc/game/version_manifest_v2.json"
        }
        ManifestTypes::PrismVersionManifest => {
            "https://meta.prismlauncher.org/v1/net.minecraft/index.json"
        }
    };

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
