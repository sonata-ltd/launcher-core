use std::path::Path;
use thiserror::Error;

use async_std::{fs::{self, File}, io::WriteExt};

use crate::utils::download::download_in_json;

#[derive(Error, Debug)]
pub enum DownloadError {
    #[error("Failed to write file or create directory: {0}. Target path: {1}")]
    WriteFailed(String, String),

    #[error("Download failed: {0}")]
    OtherError(String)
}

pub async fn download_manifest<P>(
    url: &str,
    save_path: Option<P>
) -> Result<(serde_json::Value, Option<String>), DownloadError>
where
    P: AsRef<Path>
{
    match download_in_json(url).await {
        Ok(data) => {
            if let Some(save_path) = save_path {
                let name_start_pos = url.rfind('/').unwrap();
                let full_path = format!("{}{}", save_path.as_ref().display(), url[name_start_pos..].to_string());
                let dir_last_pos = full_path.rfind('/').unwrap();

                match fs::create_dir_all(full_path[..dir_last_pos].to_string()).await {
                    Ok(_) => {
                        let mut index_file = File::create(&full_path).await.unwrap();
                        index_file.write_all(serde_json::to_string_pretty(&data).unwrap().as_bytes()).await.unwrap();
                    },
                    Err(e) => return Err(DownloadError::WriteFailed(e.to_string(), full_path)),
                }

                return Ok((data, Some(full_path)));
            }

            return Ok((data, None))
        },
        Err(e) => return Err(DownloadError::OtherError(e.to_string())),
    }
}

pub async fn get_assets_manifest<'a>(
    version_manifest: &'a serde_json::Value,
    assets_path: &'a str
) -> Result<(serde_json::Value, &'a str), String> {
    if let Some(asset_index) = version_manifest["assetIndex"].as_object() {
        if let Some(asset_url) = asset_index["url"].as_str() {
            match download_manifest(&asset_url.to_string(), Some(&assets_path)).await {
                Ok(manifest) => return Ok((manifest.0, asset_index["id"].as_str().unwrap())),
                Err(e) => return Err(format!("Failed to download assets manifest: {}", e)),
            };
        }
    }

    Err(format!("Failed to parse version manifest"))
}

pub async fn is_asset_downloaded(manifest: &serde_json::Value, k: &String, v: &serde_json::Value) -> bool {
    if let Some(_hash) = v.get("hash") {
        if let Some(assets) = manifest["assets"].as_array() {
            for asset in assets {
                if let Some(asset_name) = asset["name"].as_str() {
                    println!("asset_name: {:#?} | k: {}, | v: {}", asset_name, k, v);
                } else {
                    println!("Asset not found");
                }
            }
        } else {
            println!("Assets array not found");
        }
    }

    true
}

pub fn is_array_exists(metacache: &serde_json::Value, key: &str) -> bool {
    metacache.get(key).map_or(false, |v| v.is_array())
}
