pub mod http;
use std::ffi::{OsStr, OsString};
use std::path::Path;
use std::time::Duration;
use std::{path::PathBuf, sync::Arc};

use crate::utils::cancellation::Cancellation;
use async_std::fs::{create_dir_all, File};
use async_std::future::timeout;
#[cfg(test)]
use sha1::Digest;
use sha1::Sha1;
use uuid::Uuid;

use crate::utils::download::buffer::BufferPool;
use crate::utils::download::error::{DownloadError, Result};

pub mod buffer;
pub mod error;
pub mod json;
pub(crate) mod metadata_cache;
pub(crate) mod stream;
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

/// Version and asset manifests can exceed the provider catalog response limit.
pub async fn download_in_json(url: &str) -> Result<serde_json::Value> {
    use http::HttpError;

    const MAX_MANIFEST_BYTES: usize = 32 * 1024 * 1024;
    let request = http::request(url).map_err(|error| match error {
        HttpError::InvalidUrl(source) => DownloadError::InvalidUrl {
            url: url.to_owned(),
            source,
        },
        other => DownloadError::Request {
            url: url.to_owned(),
            source: other.into(),
        },
    })?;
    let response = timeout(REQUEST_TIMEOUT, http::send(request))
        .await
        .map_err(|_| DownloadError::Timeout {
            url: url.to_owned(),
            phase: "response",
            after: REQUEST_TIMEOUT,
        })?
        .map_err(|error| DownloadError::Request {
            url: url.to_owned(),
            source: error.into(),
        })?;
    let bytes = timeout(
        ONESHOT_TIMEOUT,
        http::read_response(response, MAX_MANIFEST_BYTES),
    )
    .await
    .map_err(|_| DownloadError::Timeout {
        url: url.to_owned(),
        phase: "body",
        after: ONESHOT_TIMEOUT,
    })?
    .map_err(|error| match error {
        HttpError::Status(status) => DownloadError::Http {
            status,
            url: url.to_owned(),
        },
        HttpError::Body(source) => DownloadError::BodyRead {
            url: url.to_owned(),
            source,
        },
        HttpError::SizeLimit(limit) => DownloadError::SizeLimit {
            url: url.to_owned(),
            limit: limit as u64,
        },
        other => DownloadError::Request {
            url: url.to_owned(),
            source: other.into(),
        },
    })?;
    serde_json::from_slice(&bytes).map_err(|source| DownloadError::MalformedJson {
        url: url.to_owned(),
        source: source.into(),
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
        self.download_with_cancellation(&Cancellation::new()).await
    }

    pub(crate) async fn download_with_cancellation(self, cancel: &Cancellation) -> Result<T> {
        cancel.check()?;
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

        let temporary = tempfile::TempPath::from_path(temp_path(save_dir, file_name));
        self.fetch_to(&temporary, cancel).await?;
        cancel.check()?;
        let final_path = self.save_path;
        async_std::task::spawn_blocking(move || {
            temporary
                .persist(&final_path)
                .map_err(|error| DownloadError::FileCommit {
                    from: error.path.to_path_buf(),
                    to: final_path,
                    source: error.error,
                })
        })
        .await?;
        Ok(self.object)
    }

    async fn fetch_to(&self, temp_path: &Path, cancel: &Cancellation) -> Result<()> {
        let (mut response, url) =
            stream::response(self.object.get_url(), false, None, cancel).await?;
        tracing::debug!(name = %self.object.get_name(), %url, "downloading");
        let mut file =
            File::create(temp_path)
                .await
                .map_err(|source| DownloadError::FileCreation {
                    path: temp_path.to_owned(),
                    source,
                })?;

        let mut guard = cancel.run(self.buffers_pool.acquire()).await?;
        let actual = stream::Stream {
            url: url.as_str(),
            destination: temp_path,
            length: stream::Length::UpTo(u64::MAX),
            cancel,
        }
        .copy::<Sha1>(&mut response, &mut file, guard.as_mut_slice(), |_| {})
        .await?;
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

pub mod transfer;
