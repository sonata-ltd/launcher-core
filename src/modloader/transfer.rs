use super::{
    error::{ModloaderError, Result},
    files,
};
use async_std::{fs, task};
use futures::{AsyncReadExt, AsyncWriteExt};
use sha1::{Digest, Sha1};
use std::{
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};

#[derive(Clone, Default)]
pub struct Cancel(pub Arc<AtomicBool>);

impl Cancel {
    pub fn check(&self) -> Result<()> {
        if self.0.load(Ordering::Acquire) {
            Err(ModloaderError::Cancelled)
        } else {
            Ok(())
        }
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }
}

pub async fn download(
    url: &str,
    destination: &Path,
    expected: Option<&str>,
    cancel: &Cancel,
) -> Result<String> {
    if let Some(hash) = expected {
        if !files::hash_valid(hash, 40) {
            return Err(ModloaderError::Invalid("invalid SHA-1".into()));
        }
    }

    cancel.check()?;

    if let Some(hash) = expected {
        let path = destination.to_owned();
        let hash = hash.to_owned();

        if task::spawn_blocking(move || files::matches(&path, &hash)).await? {
            return Ok(expected.unwrap().to_owned());
        }
    }

    let parent = destination
        .parent()
        .ok_or_else(|| ModloaderError::Invalid("download path has no parent".into()))?;

    fs::create_dir_all(parent).await?;

    let temporary = destination.with_extension(format!("{}.part", uuid::Uuid::new_v4()));

    let mut last = None;
    for attempt in 0..3u32 {
        cancel.check()?;

        let result = async {
            let mut current =
                http_types::Url::parse(url).map_err(|e| ModloaderError::Invalid(e.to_string()))?;

            let mut redirects = 0;
            let mut response = loop {
                cancel.check()?;

                let response = async_std::future::timeout(
                    Duration::from_secs(30),
                    surf::get(current.as_str())
                        .header("User-Agent", "SonataLauncher/0.1")
                        .header("Accept-Encoding", "identity"),
                )
                .await
                .map_err(|_| ModloaderError::Http("download response timeout".into()))?
                .map_err(|e| ModloaderError::Http(e.to_string()))?;

                if !response.status().is_redirection() {
                    break response;
                }

                if redirects >= 20 {
                    return Err(ModloaderError::Http("too many redirects".into()));
                }

                let location = response
                    .header("Location")
                    .map(|h| h.last().as_str())
                    .ok_or_else(|| ModloaderError::Http("redirect without Location".into()))?;

                current = current
                    .join(location)
                    .map_err(|e| ModloaderError::Http(e.to_string()))?;

                redirects += 1;
            };

            if !response.status().is_success() {
                return Err(ModloaderError::Http(format!(
                    "{}: {url}",
                    response.status()
                )));
            }

            let mut reader = response.take_body().into_reader();
            let mut output = fs::File::create(&temporary).await?;
            let mut buffer = vec![0u8; 65536];
            let mut digest = Sha1::new();
            let mut size = 0u64;

            loop {
                cancel.check()?;

                let count =
                    async_std::future::timeout(Duration::from_secs(30), reader.read(&mut buffer))
                        .await
                        .map_err(|_| ModloaderError::Http("download body timeout".into()))??;

                if count == 0 {
                    break;
                }

                size += count as u64;

                if size > 2 * 1024 * 1024 * 1024 {
                    return Err(ModloaderError::Invalid("artifact exceeds 2 GiB".into()));
                }

                digest.update(&buffer[..count]);
                output.write_all(&buffer[..count]).await?;
            }

            let hash = format!("{:x}", digest.finalize());
            if expected.is_some_and(|expected| !expected.eq_ignore_ascii_case(&hash)) {
                return Err(ModloaderError::Invalid(format!(
                    "artifact checksum mismatch: {url}"
                )));
            }

            output.sync_all().await?;
            drop(output);
            cancel.check()?;

            let source = temporary.clone();
            let target = destination.to_owned();

            task::spawn_blocking(move || {
                tempfile::TempPath::from_path(source)
                    .persist(target)
                    .map_err(|error| error.error)
            })
            .await?;

            Ok(hash)
        }
        .await;

        match result {
            Ok(hash) => return Ok(hash),
            Err(error) => {
                let _ = fs::remove_file(&temporary).await;

                if matches!(
                    error,
                    ModloaderError::Cancelled | ModloaderError::Invalid(_)
                ) {
                    return Err(error);
                }

                last = Some(error);
            }
        }

        task::sleep(Duration::from_millis(500 * 2u64.pow(attempt))).await;
    }

    Err(last.unwrap_or_else(|| ModloaderError::Http("download failed".into())))
}

pub async fn place(
    url: &str,
    path: &Path,
    expected: Option<&str>,
    blobs: &Path,
    cancel: &Cancel,
) -> Result<String> {
    let index = blobs.join("urls").join(files::sha256(url.as_bytes()));
    let cached = async_std::fs::read_to_string(&index)
        .await
        .ok()
        .filter(|s| files::hash_valid(s, 40));

    let expected = expected.or(cached.as_deref());

    if let Some(hash) = expected {
        let blob = blobs.join(hash.to_ascii_lowercase());
        let source = blob.clone();
        let hash = hash.to_owned();
        if task::spawn_blocking(move || files::matches(&source, &hash)).await? {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).await?;
            }
            fs::copy(&blob, path).await?;
            return Ok(expected.unwrap().into());
        }
    }

    let hash = download(url, path, expected, cancel).await?;

    fs::create_dir_all(blobs).await?;

    let blob = blobs.join(&hash);
    cache_blob(path, &blob).await?;

    let body = hash.as_bytes().to_vec();
    async_std::task::spawn_blocking(move || files::atomic_write(&index, &body)).await?;

    Ok(hash)
}

// The downloaded source has already passed checksum verification. Persist
// atomically replaces a corrupt blob and handles concurrent identical writers.
async fn cache_blob(source: &Path, destination: &Path) -> Result<()> {
    let temporary = destination.with_extension(format!("{}.part", uuid::Uuid::new_v4()));
    if let Err(error) = fs::copy(source, &temporary).await {
        let _ = fs::remove_file(&temporary).await;
        return Err(error.into());
    }
    let destination = destination.to_owned();
    task::spawn_blocking(move || {
        tempfile::TempPath::from_path(temporary)
            .persist(destination)
            .map_err(|error| error.error)
    })
    .await?;
    Ok(())
}

pub async fn check_file(path: PathBuf, expected: String) -> Result<()> {
    if !task::spawn_blocking(move || files::matches(&path, &expected)).await? {
        return Err(ModloaderError::Invalid(
            "runtime file missing or corrupt; repair installation".into(),
        ));
    }

    Ok(())
}

#[cfg(test)]
mod cache_tests {
    use super::*;
    #[async_std::test]
    async fn corrupted_blob_is_replaced_and_next_placement_needs_no_network() {
        let temp = tempfile::tempdir().unwrap();
        let blobs = temp.path().join("blobs");
        fs::create_dir_all(&blobs).await.unwrap();
        let source = temp.path().join("verified.jar");
        fs::write(&source, b"verified content").await.unwrap();
        let hash = files::sha1_file(&source).unwrap();
        let blob = blobs.join(&hash);
        fs::write(&blob, b"corrupt").await.unwrap();
        cache_blob(&source, &blob).await.unwrap();
        assert!(files::matches(&blob, &hash).unwrap());
        let destination = temp.path().join("next/library.jar");
        place(
            "not-a-network-url",
            &destination,
            Some(&hash),
            &blobs,
            &Cancel::default(),
        )
        .await
        .unwrap();
        assert!(files::matches(&destination, &hash).unwrap());
    }
}
