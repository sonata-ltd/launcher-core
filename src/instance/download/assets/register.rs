use super::*;

impl<'a> AssetsData<'a> {
    pub async fn register_assets(
        &self,
        mut metacache: serde_json::Value,
        downloaded_assets: &mut HashSet<AssetInfo>,
    ) {
        if let Some(assets) = metacache["assets"].as_array_mut() {
            for item in downloaded_assets.iter() {
                assets.push(json!({
                    "name": item.name,
                    "hash": item.hash,
                }));
            }
        } else {
            println!("Failed to find \"assets\" array in metacache file");
            return;
        }

        let mut metacache_file = File::create(&self.metacache_file_path).await.unwrap();

        metacache_file
            .write_all(serde_json::to_string_pretty(&metacache).unwrap().as_bytes())
            .await
            .unwrap();
    }
}
