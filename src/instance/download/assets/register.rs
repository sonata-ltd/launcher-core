use std::collections::HashSet;

use super::*;

impl<'a> AssetsData<'a> {
    pub async fn register_assets(
        &self,
        downloaded_assets: &mut HashSet<AssetInfo>,
    ) -> Result<(), AssetSyncError> {
        for item in downloaded_assets.iter() {
            let req = sqlx::query!(
                r#"
                INSERT INTO assets (name, hash, url)
                VALUES (?1, ?2, ?3)
                RETURNING id
                "#,
                item.name,
                item.hash,
                item.url
            )
            .fetch_one(&self.db.pool)
            .await
            .map_err(|e| AssetSyncError::RegisterFailed(e.to_string()))?;

            println!("Registered asset: {}", req.id);
        }

        Ok(())
    }
}
