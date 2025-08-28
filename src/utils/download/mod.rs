use std::{
    path::PathBuf,
    sync::Arc,
};

use async_std::{
    fs::{create_dir_all, File},
};
use futures::{AsyncReadExt, AsyncWriteExt};
use sha1::{Digest, Sha1};
use surf::{self, Error, Url};

use crate::utils::download::buffer::BufferPool;

pub mod buffer;
#[cfg(test)]
mod tests;

pub const MAX_REDIRECT_COUNT: usize = 100;

pub async fn download(url: String) -> Result<Vec<u8>, String> {
    match surf::get(url).recv_bytes().await {
        Ok(response) => return Ok(response),
        Err(e) => return Err(e.to_string()),
    }
}

pub async fn download_in_json<'a>(url: &'a str) -> Result<serde_json::Value, Error> {
    match surf::get(url).await {
        Ok(mut response) => match response.body_json::<serde_json::Value>().await {
            Ok(data) => Ok(data),
            Err(e) => Err(e),
        },
        Err(e) => return Err(e),
    }
}

pub struct Download<T: Downloadable> {
    save_path: PathBuf,
    object: T,
    buffers_pool: Arc<BufferPool>,
}

pub trait Downloadable {
    fn get_name(&self) -> &String;
    fn get_hash(&self) -> &String;
    fn get_url(&self) -> &String;
}

impl<T: Downloadable + Send + Sync + 'static> Download<T> {
    pub fn new(
        save_path: PathBuf,
        object: T,
        buffers_pool: Arc<BufferPool>,
    ) -> Download<T> {
        Download {
            save_path,
            object,
            buffers_pool,
        }
    }

    pub async fn download_with_checksum(self) -> Result<T, String> {
        let save_dir = match self.save_path.parent() {
            Some(dir) => dir,
            None => return Err("Cannot get parent folder of the save path".to_string())
        };

        let file_name = match self.save_path.file_name() {
            Some(file_name) => file_name,
            None => return Err("Cannot get file name of the save path".to_string())
        };

        if let Err(e) = create_dir_all(&save_dir).await {
            println!("Failed to create directory: {e}");
            return Err(e.to_string());
        }

        let mut current_url = self.object.get_url().to_owned();
        let mut redirect_count: usize = 0;

        loop {
            // Ask for identity encoding
            let req = surf::get(&current_url.to_string()).header("Accept-Encoding", "identity");
            let mut resp = match req.await {
                Ok(data) => data,
                Err(e) => return Err(format!("Request error for {}: {}", current_url, e)),
            };

            // Handle redirect from server
            if resp.status().is_redirection() {
                let status = resp.status();
                let location = resp.header("Location").map(|v| v.last());

                if redirect_count >= MAX_REDIRECT_COUNT {
                    return Err(format!("Too many redirects when fetcing {}", current_url));
                }

                let location = match location {
                    Some(loc) => loc.as_str(),
                    None => {
                        return Err(format!(
                            "Redirect (status {}) without Location for {}.",
                            status, current_url
                        ));
                    }
                };

                // Resolve relative Location
                let base = Url::parse(&current_url)
                    .map_err(|e| format!("Base URL parse error {}: {}", current_url, e))?;
                let next_url = match Url::parse(&location) {
                    Ok(u) => u, // Absolute
                    Err(_) => base
                        .join(&location)
                        .map_err(|e| format!("Failed to join {} + {}: {}", base, location, e))?,
                };

                current_url = next_url.to_string();
                redirect_count += 1;

                continue;
            }

            // Not a redirect -> proceed to download
            println!(
                "Downloading \"{}\" from URL {}",
                self.object.get_name(),
                current_url
            );

            if !resp.status().is_success() {
                return Err(format!(
                    "HTTP error {} when fetching {}",
                    resp.status(),
                    current_url
                ));
            }

            // Prepare file
            let full_save_path = save_dir.join(file_name);
            let mut file = match File::create(&full_save_path).await {
                Ok(f) => {
                    println!("Saving to: {}", full_save_path.display());
                    f
                }
                Err(e) => return Err(format!("Failed to create file {}: {}", full_save_path.display(), e)),
            };

            // Stream -> hasher + file at once with bytes read logging
            let mut hasher = Sha1::new();
            let mut guard = self.buffers_pool.acquire().await;
            let buf = guard.as_mut_slice();
            let mut total_read: usize = 0;
            loop {
                let n = resp.read(buf).await.map_err(|e| e.to_string())?;
                if n == 0 {
                    break;
                }

                hasher.update(&buf[..n]);
                file.write_all(&buf[..n]).await.map_err(|e| e.to_string())?;
                total_read += n;
            }

            if total_read == 0 {
                return Err(format!("Read 0 bytes from {}", current_url));
            }

            let calculated_sha1 = format!("{:x}", hasher.finalize());
            let expected = self.object.get_hash().to_lowercase();

            if calculated_sha1 != expected {
                return Err(format!("SHA1 mismatch at {}", current_url));
            } else {
                return Ok(self.object);
            }
        }
    }
}
