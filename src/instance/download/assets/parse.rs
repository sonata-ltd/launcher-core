use super::*;

const ASSETS_BASE_URL: &'static str = "https://resources.download.minecraft.net";

pub fn extract_assets(manifest: &serde_json::Value) -> Result<Vec<AssetInfo>, AssetSyncError> {
    let objects = manifest["objects"]
        .as_object()
        .ok_or(AssetSyncError::ManifestMalformed)?;

    let mut assets = Vec::with_capacity(objects.len());

    for (name, value) in objects {
        let hash = value["hash"]
            .as_str()
            .filter(|hash| crate::utils::files::hash_valid(hash, 40))
            .ok_or_else(|| AssetSyncError::InvalidHash(name.clone()))?
            .to_ascii_lowercase();

        assets.push(AssetInfo {
            name: name.clone(),
            url: format!("{}/{}/{}", ASSETS_BASE_URL, &hash[..2], hash),
            hash,
        });
    }

    Ok(assets)
}
