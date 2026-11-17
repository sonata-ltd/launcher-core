use async_std::{fs::File, future::timeout};
use futures::{AsyncReadExt, AsyncWriteExt};
use sha2::{Digest, Sha256};
use std::{path::Path, time::Duration};

use super::model::*;
use crate::utils::cancellation::Cancellation;

pub(super) async fn download(
    package: &ManagedJavaPackage,
    path: &Path,
    cancel: &Cancellation,
    progress: impl Fn(u64),
) -> Result<String> {
    let mut url =
        surf::Url::parse(&package.url).map_err(|e| ManagedJavaError::new("download", e))?;
    let mut redirects = 0;
    let mut response = loop {
        if url.scheme() != "https" {
            return Err(ManagedJavaError::new(
                "download",
                "Java downloads require HTTPS",
            ));
        }
        let response = cancel
            .run(async {
                timeout(
                    Duration::from_secs(30),
                    surf::get(url.as_str()).header("Accept-Encoding", "identity"),
                )
                .await
                .map_err(|_| {
                    ManagedJavaError::new("download", format!("Response timed out: {url}"))
                })?
                .map_err(|e| ManagedJavaError::new("download", format!("{url}: {e}")))
            })
            .await??;
        if !response.status().is_redirection() {
            break response;
        }
        if redirects >= 20 {
            return Err(ManagedJavaError::new("download", "Too many redirects"));
        }
        let location = response
            .header("Location")
            .map(|v| v.last().as_str())
            .ok_or_else(|| ManagedJavaError::new("download", "Redirect has no Location"))?;
        url = url
            .join(location)
            .map_err(|e| ManagedJavaError::new("download", e))?;
        redirects += 1;
    };
    if !response.status().is_success() {
        return Err(ManagedJavaError::new(
            "download",
            format!("HTTP {}: {url}", response.status()),
        ));
    }
    let mut file = File::create(path)
        .await
        .map_err(|e| ManagedJavaError::new("download", format!("{}: {e}", path.display())))?;
    stream_body(&mut response, &mut file, package.size, cancel, progress).await
}

pub(super) async fn stream_body(
    reader: &mut (impl futures::AsyncRead + Unpin),
    writer: &mut (impl futures::AsyncWrite + Unpin),
    expected_size: u64,
    cancel: &Cancellation,
    progress: impl Fn(u64),
) -> Result<String> {
    let mut hash = Sha256::new();
    let mut buffer = vec![0; 64 * 1024];
    let mut downloaded = 0;
    loop {
        let n = cancel
            .run(async {
                timeout(Duration::from_secs(30), reader.read(&mut buffer))
                    .await
                    .map_err(|_| {
                        ManagedJavaError::new("download", "Reading response body timed out")
                    })?
                    .map_err(|e| ManagedJavaError::new("download", e))
            })
            .await??;
        if n == 0 {
            break;
        }
        downloaded += n as u64;
        if downloaded > expected_size {
            return Err(ManagedJavaError::new(
                "download",
                format!("Expected {expected_size} bytes; received more than expected"),
            ));
        }
        cancel
            .run(async {
                writer
                    .write_all(&buffer[..n])
                    .await
                    .map_err(|e| ManagedJavaError::new("download", e))
            })
            .await??;
        hash.update(&buffer[..n]);
        progress(downloaded);
    }
    if downloaded != expected_size {
        return Err(ManagedJavaError::new(
            "download",
            format!("Expected {expected_size} bytes; received {downloaded}"),
        ));
    }
    writer
        .flush()
        .await
        .map_err(|e| ManagedJavaError::new("download", e))?;
    Ok(format!("{:x}", hash.finalize()))
}
