use std::ffi::{OsStr, OsString};
use std::path::Path;
use std::time::Duration;
use std::{path::PathBuf, sync::Arc};

use async_std::fs::{create_dir_all, remove_file, rename, File};
use async_std::future::timeout;
use futures::{AsyncReadExt, AsyncWriteExt};
use sha1::{Digest, Sha1};
use surf::{self, Url};
use uuid::Uuid;

use crate::utils::download::buffer::BufferPool;
use crate::utils::download::error::{DownloadError, Result};

pub mod buffer;
pub mod error;
pub mod sweep;
#[cfg(test)]
mod tests;

pub const MAX_REDIRECT_COUNT: usize = 20;

pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

pub const READ_TIMEOUT: Duration = Duration::from_secs(30);

pub const ONESHOT_TIMEOUT: Duration = Duration::from_secs(60);

pub async fn download(url: String) -> Result<Vec<u8>> {
    let fetch = timeout(ONESHOT_TIMEOUT, surf::get(&url).recv_bytes())
        .await
        .map_err(|_| DownloadError::Timeout {
            url: url.clone(),
            phase: "response",
            after: ONESHOT_TIMEOUT,
        })?;

    fetch.map_err(|e| DownloadError::Request {
        url,
        source: e.into(),
    })
}

pub async fn download_in_json<'a>(url: &'a str) -> Result<serde_json::Value> {
    let mut response = timeout(REQUEST_TIMEOUT, surf::get(url))
        .await
        .map_err(|_| DownloadError::Timeout {
            url: url.to_string(),
            phase: "response",
            after: REQUEST_TIMEOUT,
        })?
        .map_err(|e| DownloadError::Request {
            url: url.to_string(),
            source: e.into(),
        })?;

    timeout(ONESHOT_TIMEOUT, response.body_json::<serde_json::Value>())
        .await
        .map_err(|_| DownloadError::Timeout {
            url: url.to_string(),
            phase: "body",
            after: ONESHOT_TIMEOUT,
        })?
        .map_err(|e| DownloadError::MalformedJson {
            url: url.to_string(),
            source: e.into(),
        })
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
    pub fn new(save_path: PathBuf, object: T, buffers_pool: Arc<BufferPool>) -> Download<T> {
        Download {
            save_path,
            object,
            buffers_pool,
        }
    }

    pub async fn download_with_checksum(self) -> Result<T> {
        let (save_dir, file_name) = match (self.save_path.parent(), self.save_path.file_name()) {
            (Some(dir), Some(name)) => (dir, name),
            _ => {
                return Err(DownloadError::InvalidSavePath {
                    path: self.save_path.clone(),
                })
            }
        };

        create_dir_all(save_dir)
            .await
            .map_err(|e| DownloadError::DirectoryCreation {
                path: save_dir.to_path_buf(),
                source: e,
            })?;

        let temp_path = temp_path(save_dir, file_name);

        if let Err(e) = self.fetch_to(&temp_path).await {
            if let Err(cleanup) = remove_file(&temp_path).await {
                tracing::warn!(path = %temp_path.display(), error = %cleanup, "failed to remove partial download");
            }

            return Err(e);
        }

        rename(&temp_path, &self.save_path)
            .await
            .map_err(|e| DownloadError::FileCommit {
                from: temp_path,
                to: self.save_path.clone(),
                source: e,
            })?;

        Ok(self.object)
    }

    async fn fetch_to(&self, temp_path: &Path) -> Result<()> {
        let mut url = Url::parse(self.object.get_url()).map_err(|e| DownloadError::InvalidUrl {
            url: self.object.get_url().clone(),
            source: e,
        })?;

        let mut redirects: usize = 0;

        let mut resp = loop {
            let resp = timeout(
                REQUEST_TIMEOUT,
                surf::get(&url).header("Accept-Encoding", "identity"),
            )
            .await
            .map_err(|_| DownloadError::Timeout {
                url: url.to_string(),
                phase: "response",
                after: REQUEST_TIMEOUT,
            })?
            .map_err(|e| DownloadError::Request {
                url: url.to_string(),
                source: e.into(),
            })?;

            let status = resp.status();

            if !status.is_redirection() {
                break resp;
            }

            if redirects >= MAX_REDIRECT_COUNT {
                return Err(DownloadError::TooManyRedirects {
                    url: url.to_string(),
                    count: redirects,
                });
            }

            let location = resp
                .header("Location")
                .map(|v| v.last().as_str().to_owned())
                .ok_or_else(|| DownloadError::RedirectWithoutLocation {
                    status,
                    url: url.to_string(),
                })?;

            let next = url.join(&location);
            let base = url.to_string();

            url = next.map_err(|e| DownloadError::RedirectTargetInvalid {
                base,
                location,
                source: e,
            })?;

            redirects += 1;
        };

        let status = resp.status();
        if !status.is_success() {
            return Err(DownloadError::Http {
                status,
                url: url.to_string(),
            });
        }

        tracing::debug!(name = %self.object.get_name(), %url, "downloading");

        let mut file = File::create(temp_path)
            .await
            .map_err(|e| DownloadError::FileCreation {
                path: temp_path.to_path_buf(),
                source: e,
            })?;

        let mut hasher = Sha1::new();
        let mut guard = self.buffers_pool.acquire().await;
        let buf = guard.as_mut_slice();

        loop {
            let n = timeout(READ_TIMEOUT, resp.read(buf))
                .await
                .map_err(|_| DownloadError::Timeout {
                    url: url.to_string(),
                    phase: "body chunk",
                    after: READ_TIMEOUT,
                })?
                .map_err(|e| DownloadError::BodyRead {
                    url: url.to_string(),
                    source: e,
                })?;

            if n == 0 {
                break;
            }

            hasher.update(&buf[..n]);

            file.write_all(&buf[..n])
                .await
                .map_err(|e| DownloadError::FileWrite {
                    path: temp_path.to_path_buf(),
                    source: e,
                })?;
        }

        let actual = format!("{:x}", hasher.finalize());
        let expected = self.object.get_hash().to_lowercase();

        if actual != expected {
            return Err(DownloadError::ChecksumMismatch {
                url: url.to_string(),
                expected,
                actual,
            });
        }

        file.sync_all()
            .await
            .map_err(|e| DownloadError::FileWrite {
                path: temp_path.to_path_buf(),
                source: e,
            })?;

        drop(file);

        Ok(())
    }
}

fn temp_path(save_dir: &Path, file_name: &OsStr) -> PathBuf {
    let mut name = OsString::with_capacity(file_name.len() + 46);

    name.push(".");
    name.push(file_name);
    name.push(format!(".{}.part", Uuid::new_v4().simple()));

    save_dir.join(name)
}
