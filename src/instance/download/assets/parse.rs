use std::{collections::HashSet, path::PathBuf, sync::Arc};

use async_std::task;

use crate::utils::download::{buffer::BufferPool, Download};

use super::*;

const ASSETS_BASE_URL: &'static str = "https://resources.download.minecraft.net";
const CONCURRENT_TASKS_COUNT: usize = 100;
const CONCURRENT_BUFFERS_SIZE: usize = 16 * 1024; // 16 KiB each

impl<'a> AssetsData<'a> {
    pub async fn extract_manifest_assets(&self) -> Result<(), String> {
        let mut downloaded_assets: HashSet<AssetInfo> = HashSet::new();
        let download_buffer_pool = Arc::new(BufferPool::new(
            CONCURRENT_TASKS_COUNT,
            CONCURRENT_BUFFERS_SIZE
        ));

        let assets_dir_pathbuf = PathBuf::from(&self.assets_path);
        let assets_dir = Arc::new(&assets_dir_pathbuf);

        // Retrieve downloaded assets from db
        let cached = match sqlx::query_as!(
            AssetInfo,
            r#"SELECT name, hash, url
            FROM assets
            "#
        )
        .fetch_all(&self.db.pool)
        .await
        {
            Ok(cached) => cached,
            Err(e) => return Err(e.to_string()),
        };

        if let Some(objects) = self.manifest["objects"].as_object() {
            let mut futures = FuturesUnordered::new();
            println!("Checking for assets...");

            for (name, v) in objects {
                let hash = match v["hash"].as_str() {
                    Some(h) => h,
                    None => continue,
                };

                if cached.contains(&AssetInfo::with_hash(hash)) {
                    continue;
                }

                let name = name.to_string();
                let (url, relative_save_path) = Self::construct_asset_url(hash);
                let save_path = assets_dir.join(relative_save_path);

                let asset_info = AssetInfo {
                    name,
                    hash: hash.to_string(),
                    url
                };

                let dl = Download::new(save_path, asset_info, Arc::clone(&download_buffer_pool));

                futures.push(task::spawn(async move {
                    match dl.download_with_checksum().await {
                        Ok(asset_info) => Some(asset_info),
                        Err(e) => {
                            println!("{e}");
                            None
                        }
                    }
                }));

                if futures.len() >= 100 {
                    Self::process_futures(
                        &mut futures,
                        &mut downloaded_assets,
                        objects.len(),
                        Arc::clone(&self.ws_status),
                    )
                    .await;
                }
            }

            Self::process_futures(
                &mut futures,
                &mut downloaded_assets,
                objects.len(),
                Arc::clone(&self.ws_status),
            )
            .await;
        }

        match Self::register_assets(&self, &mut downloaded_assets).await {
            Ok(_) => Ok(()),
            Err(e) => Err(e.to_string())
        }
    }

    /// Construct the url by hash and domain
    /// Retrieves url to download and relative path for a file saving
    fn construct_asset_url(hash: &str) -> (String, String) {
        let short_hash = &hash[..2];

        (
            format!("{}/{}/{}", ASSETS_BASE_URL, short_hash, hash),
            format!("{}/{}", short_hash, hash)
        )
    }
}
