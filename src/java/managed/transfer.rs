use async_std::{fs::File, future::timeout};
use futures::{
    future::{select, Either},
    AsyncReadExt, AsyncWriteExt,
};
use sha2::{Digest, Sha256};
use std::{
    future::Future,
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};

use super::model::*;

#[derive(Clone)]
pub(super) struct Cancellation {
    flag: Arc<AtomicBool>,
    tx: async_channel::Sender<()>,
    rx: async_channel::Receiver<()>,
}

impl Cancellation {
    pub fn new() -> Self {
        let (tx, rx) = async_channel::bounded(1);
        Self {
            flag: Arc::new(AtomicBool::new(false)),
            tx,
            rx,
        }
    }

    pub fn cancel(&self) {
        self.flag.store(true, Ordering::SeqCst);
        let _ = self.tx.try_send(());
    }

    pub fn check(&self) -> Result<()> {
        if self.flag.load(Ordering::SeqCst) {
            Err(ManagedJavaError::new("cancelled", "Cancellation requested"))
        } else {
            Ok(())
        }
    }

    pub async fn wait<T>(&self, future: impl Future<Output = Result<T>>) -> Result<T> {
        self.check()?;
        match select(Box::pin(future), Box::pin(self.rx.recv())).await {
            Either::Left((result, _)) => result,
            Either::Right(_) => Err(ManagedJavaError::new("cancelled", "Cancellation requested")),
        }
    }
}

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
            .wait(async {
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
            .await?;
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
            .wait(async {
                timeout(Duration::from_secs(30), reader.read(&mut buffer))
                    .await
                    .map_err(|_| {
                        ManagedJavaError::new("download", "Reading response body timed out")
                    })?
                    .map_err(|e| ManagedJavaError::new("download", e))
            })
            .await?;
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
            .wait(async {
                writer
                    .write_all(&buffer[..n])
                    .await
                    .map_err(|e| ManagedJavaError::new("download", e))
            })
            .await?;
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
