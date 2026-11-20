pub mod http;
use std::ffi::{OsStr, OsString};
use std::path::Path;
use std::time::Duration;
use std::{path::PathBuf, sync::Arc};

use crate::utils::cancellation::Cancellation;
use async_std::fs::File;
use async_std::future::timeout;
use async_std::task;
#[cfg(test)]
use sha1::Digest;
use sha1::Sha1;
use uuid::Uuid;

use crate::utils::download::buffer::BufferPool;
use crate::utils::download::error::{DownloadError, Result};

use self::destination::Destination;

pub mod buffer;
pub(crate) mod destination;
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
    timeout(ONESHOT_TIMEOUT, async {
        let (mut response, final_url) =
            stream::response(&url, false, None, &Cancellation::new()).await?;
        response
            .body_bytes()
            .await
            .map_err(|error| DownloadError::Request {
                url: http::diagnostic_url(final_url.as_str()),
                source: error.into(),
            })
    })
    .await
    .map_err(|_| DownloadError::Timeout {
        url: http::diagnostic_url(&url),
        phase: "response",
        after: ONESHOT_TIMEOUT,
    })?
}

/// Version and asset manifests can exceed the provider catalog response limit.
pub async fn download_in_json(url: &str) -> Result<serde_json::Value> {
    let bytes = download_manifest_bytes(url).await?;
    serde_json::from_slice(&bytes).map_err(|source| DownloadError::MalformedJson {
        url: http::diagnostic_url(url),
        source: source.into(),
    })
}

pub(crate) const MAX_MANIFEST_BYTES: usize = 32 * 1024 * 1024;

pub(crate) async fn download_manifest_bytes(url: &str) -> Result<Vec<u8>> {
    use http::HttpError;

    let request = http::request(url).map_err(|error| match error {
        HttpError::InvalidUrl(source) => DownloadError::InvalidUrl {
            url: http::diagnostic_url(url),
            source,
        },
        HttpError::UnsupportedScheme => DownloadError::UnsupportedScheme {
            url: http::diagnostic_url(url),
            https_only: false,
        },
        HttpError::UrlCredentials => DownloadError::UrlCredentials {
            url: http::diagnostic_url(url),
        },
        other => DownloadError::Request {
            url: http::diagnostic_url(url),
            source: other.into(),
        },
    })?;
    let response = timeout(REQUEST_TIMEOUT, http::send(request))
        .await
        .map_err(|_| DownloadError::Timeout {
            url: http::diagnostic_url(url),
            phase: "response",
            after: REQUEST_TIMEOUT,
        })?
        .map_err(|error| DownloadError::Request {
            url: http::diagnostic_url(url),
            source: error.into(),
        })?;
    timeout(
        ONESHOT_TIMEOUT,
        http::read_response(response, MAX_MANIFEST_BYTES),
    )
    .await
    .map_err(|_| DownloadError::Timeout {
        url: http::diagnostic_url(url),
        phase: "body",
        after: ONESHOT_TIMEOUT,
    })?
    .map_err(|error| match error {
        HttpError::Status(status) => DownloadError::Http {
            status,
            url: http::diagnostic_url(url),
        },
        HttpError::Body(source) => DownloadError::BodyRead {
            url: http::diagnostic_url(url),
            source,
        },
        HttpError::SizeLimit(limit) => DownloadError::SizeLimit {
            url: http::diagnostic_url(url),
            limit: limit as u64,
        },
        other => DownloadError::Request {
            url: http::diagnostic_url(url),
            source: other.into(),
        },
    })
}

pub struct Download<T: Downloadable> {
    destination: Destination,
    object: T,
    buffers_pool: Arc<BufferPool>,
}

pub trait Downloadable {
    fn get_name(&self) -> &String;
    fn get_hash(&self) -> &String;
    fn get_url(&self) -> &String;

    /// Identity of the download destination, not necessarily of its bytes.
    fn cache_key(&self) -> String {
        self.get_hash().clone()
    }
}

impl<T: Downloadable + Send + Sync + 'static> Download<T> {
    pub fn new(save_path: PathBuf, object: T, buffers_pool: Arc<BufferPool>) -> Download<T> {
        Self::to(save_path.into(), object, buffers_pool)
    }

    pub(crate) fn to(destination: Destination, object: T, buffers_pool: Arc<BufferPool>) -> Self {
        Download {
            destination,
            object,
            buffers_pool,
        }
    }

    pub async fn download_with_checksum(self) -> Result<T> {
        self.download_with_cancellation(&Cancellation::new()).await
    }

    pub(crate) async fn download_with_cancellation(self, cancel: &Cancellation) -> Result<T> {
        cancel.check()?;
        let Self {
            destination,
            object,
            buffers_pool,
        } = self;
        let final_path = destination.path();
        let save_dir = final_path
            .parent()
            .filter(|_| final_path.file_name().is_some())
            .ok_or_else(|| DownloadError::InvalidSavePath {
                path: final_path.clone(),
            })?;
        let target = task::spawn_blocking(move || destination.bind())
            .await
            .map_err(|source| DownloadError::DirectoryCreation {
                path: save_dir.to_owned(),
                source,
            })?;
        let final_path = target.path();
        let (target, partial) = task::spawn_blocking(move || {
            let partial = target.stage();
            (target, partial)
        })
        .await;
        let (temporary, mut output) = partial.map_err(|source| DownloadError::FileCreation {
            path: final_path.clone(),
            source,
        })?;
        Self::fetch_to(&object, &buffers_pool, &temporary.path, &mut output, cancel).await?;
        drop(output);
        cancel.check()?;
        let cancel = cancel.clone();
        task::spawn_blocking(move || -> Result<()> {
            cancel.check()?;
            let from = temporary.path.clone();
            temporary
                .publish(&target.name)
                .map_err(|source| DownloadError::FileCommit {
                    from,
                    to: final_path,
                    source,
                })
        })
        .await?;
        Ok(object)
    }

    async fn fetch_to(
        object: &T,
        buffers_pool: &BufferPool,
        temp_path: &Path,
        file: &mut File,
        cancel: &Cancellation,
    ) -> Result<()> {
        let (mut response, url) = stream::response(object.get_url(), false, None, cancel).await?;
        tracing::debug!(name = %object.get_name(), url = %http::diagnostic_url(url.as_str()), "downloading");

        let mut guard = cancel.run(buffers_pool.acquire()).await?;
        let actual = stream::Stream {
            url: url.as_str(),
            destination: temp_path,
            length: stream::Length::UpTo(u64::MAX),
            cancel,
        }
        .copy::<Sha1>(&mut response, file, guard.as_mut_slice(), |_| {})
        .await?;
        let expected = object.get_hash().to_lowercase();

        if actual != expected {
            return Err(DownloadError::ChecksumMismatch {
                url: http::diagnostic_url(url.as_str()),
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
