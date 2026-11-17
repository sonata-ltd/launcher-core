use std::{io, path::PathBuf};

use async_std::fs::create_dir_all;
use zip_extensions::zip_extract;

pub struct Natives {}

impl Natives {
    pub async fn extract(paths: Vec<PathBuf>, destination: &PathBuf) -> Result<(), io::Error> {
        tracing::debug!("paths found: {:#?}", paths);

        match create_dir_all(&destination).await {
            Ok(_) => {
                for lib_path in paths {
                    match zip_extract(&lib_path, &destination) {
                        Ok(_) => {
                            tracing::debug!(lib_path = %lib_path.display(), "native extracted")
                        }
                        Err(e) => {
                            tracing::error!(
                                "native failed to extract: {} ({})",
                                lib_path.display(),
                                e
                            )
                        }
                    }
                }
            }
            Err(e) => return Err(e),
        }

        Ok(())
    }
}
