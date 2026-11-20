use super::*;

const ASSETS_BASE_URL: &'static str = "https://resources.download.minecraft.net";

pub fn extract_assets(manifest: &serde_json::Value) -> Result<Vec<AssetInfo>, AssetSyncError> {
    let objects = manifest["objects"]
        .as_object()
        .ok_or(AssetSyncError::ManifestMalformed)?;

    let mut assets = Vec::with_capacity(objects.len());

    for (name, value) in objects {
        assets.push(asset(name.clone(), value["hash"].as_str())?);
    }

    Ok(assets)
}

pub(super) fn asset(name: String, hash: Option<&str>) -> Result<AssetInfo, AssetSyncError> {
    let Some(hash) = hash.filter(|hash| crate::utils::files::hash_valid(hash, 40)) else {
        return Err(AssetSyncError::InvalidHash(name));
    };
    let hash = hash.to_ascii_lowercase();
    Ok(AssetInfo {
        name,
        url: format!("{}/{}/{}", ASSETS_BASE_URL, &hash[..2], hash),
        hash,
    })
}
