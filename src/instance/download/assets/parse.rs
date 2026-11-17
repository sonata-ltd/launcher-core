use super::*;

const ASSETS_BASE_URL: &'static str = "https://resources.download.minecraft.net";

pub fn extract_assets(manifest: &serde_json::Value) -> Result<Vec<AssetInfo>, AssetSyncError> {
    let objects = manifest["objects"]
        .as_object()
        .ok_or(AssetSyncError::ManifestMalformed)?;

    let mut assets = Vec::with_capacity(objects.len());

    for (name, value) in objects {
        let hash = match value["hash"].as_str() {
            Some(h) if h.len() >= 2 => h,
            _ => continue,
        };

        assets.push(AssetInfo::new(
            name.clone(),
            hash.to_string(),
            format!("{}/{}/{}", ASSETS_BASE_URL, &hash[..2], hash),
        ));
    }

    Ok(assets)
}
