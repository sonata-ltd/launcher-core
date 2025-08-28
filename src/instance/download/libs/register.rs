use std::collections::HashSet;

use thiserror::Error;

use super::*;

#[derive(Error, Debug)]
pub enum RegisterError {
    #[error("Failed to register: {0}")]
    FailedToRegister(String),
}

impl<'a, 'b> LibsData<'a, 'b> {
    pub async fn register_libs(
        downloaded_libs: &mut HashSet<LibInfo>,
        db: &'a db::Database,
    ) -> Result<(Vec<String>, Vec<PathBuf>), RegisterError> {
        let mut classpaths = Vec::new();
        let mut natives_paths = Vec::new();

        for item in downloaded_libs.drain() {
            let rec = sqlx::query!(
                r#"
                INSERT INTO libraries (name, hash, path, native, url)
                VALUES (?1, ?2, ?3, ?4, ?5)
                RETURNING id
                "#,
                item.name,
                item.hash,
                item.path,
                item.native,
                item.url
            )
            .fetch_one(&db.pool)
            .await
            .map_err(|e| RegisterError::FailedToRegister(e.to_string()))?;

            classpaths.push(item.path.clone());

            if item.is_native() {
                natives_paths.push(PathBuf::from(item.path));
            }

            println!("registered lib: {}", rec.id);
        }

        Ok((classpaths, natives_paths))
    }
}
