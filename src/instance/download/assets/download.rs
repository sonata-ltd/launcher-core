use super::*;


impl<'a> AssetsData<'a> {
    pub async fn process_futures(
        futures: &mut FuturesUnordered<async_std::task::JoinHandle<std::option::Option<AssetInfo>>>,
        downloaded_assets: &mut HashSet<AssetInfo>,
        max: usize,
        ws_status: OperationWsMessageLocked<'a>,
    ) {
        let mut ws_status = ws_status;

        while let Some(result) = futures.next().await {
            if let Some(asset_info) = result {
                ws_status = ws_status
                    .update_determinable(
                        STAGE_TYPE,
                        Some(ProcessTarget::file(
                            asset_info.name.clone(),
                            FileStatus::Downloaded,
                        )),
                        downloaded_assets.len(),
                        max,
                    )
                    .await;

                downloaded_assets.insert(asset_info);
            }
        }
    }

    fn construct_asset_url(base_url: &str, hash: &String) -> (String, String, String) {
        let hash_part = &hash[0..2].to_string();

        (
            format!("{}{}/{}", base_url, hash_part, hash),
            hash_part.to_string(),
            hash.to_string(),
        )
    }

    pub async fn download_asset(
        base_url: &str,
        asset_hash: &str,
        asset_name: &str,
        path: &str,
    ) -> Result<AssetInfo, String> {
        let asset_url_data = Self::construct_asset_url(&base_url, &asset_hash.to_string());
        let (full_url, hash_part, hash) = asset_url_data;

        match create_dir_all(format!("{}/{}", path, hash_part)).await {
            Ok(_) => {}
            Err(e) => {
                println!("Failed to create dir for asset: {e}");
                return Err(e.to_string());
            }
        }

        match surf::get(&full_url).await {
            Ok(mut response) => {
                let mut file = File::create(format!("{}/{}/{}", path, hash_part, hash))
                    .await
                    .unwrap();
                async_std::io::copy(&mut response, &mut file).await.unwrap();

                Ok(AssetInfo {
                    name: asset_name.to_string(),
                    hash: asset_hash.to_string(),
                })
            }
            Err(e) => {
                println!("{e}");
                Err(e.to_string())
            }
        }
    }
}
