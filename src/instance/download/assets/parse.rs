use super::*;


impl<'a> AssetsData<'a> {
    pub async fn extract_manifest_assets(&self) {
        let base_url = "https://resources.download.minecraft.net/";
        let metacache_file = std::fs::File::open(&self.metacache_file_path).unwrap();
        let mut metacache: serde_json::Value = serde_json::from_reader(&metacache_file).unwrap();
        let mut downloaded_assets: HashSet<AssetInfo> = HashSet::new();

        if !is_array_exists(&metacache, "assets") {
            if let Some(metacache_object) = metacache.as_object_mut() {
                metacache_object.insert("assets".to_string(), json!([]));
                let mut metacache_file = std::fs::File::create(&self.metacache_file_path).unwrap();
                metacache_file
                    .write_all(serde_json::to_string_pretty(&metacache).unwrap().as_bytes())
                    .unwrap();
            }
        }

        if let Some(objects) = self.manifest["objects"].as_object() {
            if let Some(assets) = metacache["assets"].as_array() {
                let mut futures = FuturesUnordered::new();

                println!("Checking for assets...");

                for (k, v) in objects {
                    if !assets.iter().any(|asset| {
                        asset["name"].as_str() == Some(k)
                            && asset["hash"].as_str() == v["hash"].as_str()
                    }) {
                        let base_url = base_url.to_string();
                        let hash = v["hash"].as_str().unwrap().to_string();
                        let name = k.to_string();
                        let assets_path = self.assets_path.to_string();

                        futures.push(task::spawn(async move {
                            println!("Downloading asset '{}'", name);
                            match Self::download_asset(&base_url, &hash, &name, &assets_path).await
                            {
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
                }

                Self::process_futures(
                    &mut futures,
                    &mut downloaded_assets,
                    objects.len(),
                    Arc::clone(&self.ws_status),
                )
                .await;
            }
        }

        Self::register_assets(&self, metacache, &mut downloaded_assets).await;
    }
}
