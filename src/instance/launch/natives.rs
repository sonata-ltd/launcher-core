use std::path::PathBuf;

use async_std::fs::create_dir_all;
use zip_extensions::zip_extract;

pub struct Natives {}

impl Natives {
    pub async fn extract(paths: Vec<PathBuf>, destination: &PathBuf) -> Result<(), String> {
        println!("paths: {:#?}", paths);
        match create_dir_all(&destination).await {
            Ok(_) => {
                for lib_path in paths {
                    println!("{}", lib_path.display());
                    match zip_extract(&lib_path, &destination) {
                        Ok(_) => println!("Extracted: {}", lib_path.display()),
                        Err(e) => {
                            println!("{} - {}", lib_path.display(), e.to_string());
                        }
                    }
                }
            }
            Err(e) => return Err(e.to_string())
        }

        Ok(())
    }
}
