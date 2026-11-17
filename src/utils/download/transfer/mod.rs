pub mod error;
use super::{
    error::DownloadError as StreamError,
    stream::{self, Length, Stream},
};
use crate::{
    utils::download::transfer::error::{DownloadError, Result},
    utils::files,
};
use async_std::{fs, task};
#[cfg(test)]
use sha1::Digest;
use sha1::Sha1;
use std::{
    path::{Path, PathBuf},
    time::Duration,
};

#[cfg(test)]
mod tests;

pub use crate::utils::cancellation::Cancellation as Cancel;

pub async fn download(
    url: &str,
    destination: &Path,
    expected: Option<&str>,
    cancel: &Cancel,
) -> Result<String> {
    if let Some(hash) = expected {
        if !files::hash_valid(hash, 40) {
            return Err(DownloadError::Invalid("invalid SHA-1".into()));
        }
    }

    cancel.check()?;

    if let Some(hash) = expected {
        let path = destination.to_owned();
        let hash = hash.to_owned();

        let token = cancel.clone();
        let cached =
            task::spawn_blocking(move || files::matches_cancellable(&path, &hash, &token)).await;
        cancel.check()?;
        if cached? {
            return Ok(expected.unwrap().to_owned());
        }
    }

    let parent = destination
        .parent()
        .ok_or_else(|| DownloadError::Invalid("download path has no parent".into()))?;

    fs::create_dir_all(parent).await?;

    let file_name = destination
        .file_name()
        .ok_or_else(|| DownloadError::Invalid("download path has no file name".into()))?;

    const MAX_ATTEMPTS: u32 = 3;
    for attempt in 1..=MAX_ATTEMPTS {
        cancel.check()?;

        let result = async {
            const MAX_ARTIFACT_BYTES: u64 = 2 * 1024 * 1024 * 1024;
            let (mut response, final_url) =
                stream::response(url, false, Some("SonataLauncher/0.1"), cancel)
                    .await
                    .map_err(map_stream_error)?;
            let temporary = tempfile::TempPath::from_path(super::temp_path(parent, file_name));
            let mut output = fs::File::create(&temporary as &Path).await?;
            let hash = Stream {
                url: final_url.as_str(),
                destination: &temporary,
                length: Length::UpTo(MAX_ARTIFACT_BYTES),
                cancel,
            }
            .copy::<Sha1>(&mut response, &mut output, &mut vec![0; 64 * 1024], |_| {})
            .await
            .map_err(map_stream_error)?;
            if let Some(expected) =
                expected.filter(|expected| !expected.eq_ignore_ascii_case(&hash))
            {
                return Err(DownloadError::ChecksumMismatch {
                    url: url.into(),
                    expected: expected.into(),
                    actual: hash,
                });
            }

            output.sync_all().await?;
            drop(output);
            cancel.check()?;

            let target = destination.to_owned();
            task::spawn_blocking(move || temporary.persist(target).map_err(|error| error.error))
                .await?;

            Ok(hash)
        }
        .await;

        match result {
            Ok(hash) => return Ok(hash),
            Err(error) => {
                if attempt == MAX_ATTEMPTS || !error.is_retryable() {
                    return Err(error);
                }
                let delay = error.retry_delay(Duration::from_millis(500 * 2u64.pow(attempt - 1)));
                cancel.run(task::sleep(delay)).await?;
            }
        }
    }

    unreachable!("the final attempt always returns")
}

fn map_stream_error(error: StreamError) -> DownloadError {
    match error {
        StreamError::Cancelled(cancelled) => cancelled.into(),
        invalid @ StreamError::InvalidUrl { .. } => DownloadError::Invalid(invalid.to_string()),
        StreamError::Timeout { phase, .. } => DownloadError::Timeout(phase),
        StreamError::SizeLimit { limit, .. } => DownloadError::SizeLimit { limit },
        StreamError::BodyRead { source, .. } | StreamError::FileWrite { source, .. } => {
            source.into()
        }
        other => DownloadError::Network(Box::new(other)),
    }
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

    cancel.check()?;
    if let Some(hash) = expected {
        if !files::hash_valid(hash, 40) {
            return Err(DownloadError::Invalid("invalid SHA-1".into()));
        }
        let blob = blobs.join(hash.to_ascii_lowercase());
        let target = path.to_owned();
        let digest = hash.to_owned();
        let cancellation = cancel.clone();
        let placed = task::spawn_blocking(move || -> Result<bool> {
            if !files::matches_cancellable(&blob, &digest, &cancellation)? {
                return Ok(false);
            }
            files::placement::copy_replace(&blob, &target, &cancellation)?;
            Ok(true)
        })
        .await;
        match placed {
            Ok(true) => return Ok(hash.into()),
            Ok(false) => {}
            Err(DownloadError::Io(error)) if error.kind() == std::io::ErrorKind::Interrupted => {
                cancel.check()?;
                return Err(error.into());
            }
            Err(error) => return Err(error),
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
    let source = source.to_owned();
    let destination = destination.to_owned();
    task::spawn_blocking(move || {
        files::placement::copy_replace(
            &source,
            &destination,
            &crate::utils::cancellation::Cancellation::default(),
        )
    })
    .await?;
    Ok(())
}

pub async fn check_file(path: PathBuf, expected: String) -> Result<()> {
    if !task::spawn_blocking(move || files::matches(&path, &expected)).await? {
        return Err(DownloadError::Invalid(
            "runtime file missing or corrupt; repair installation".into(),
        ));
    }

    Ok(())
}

#[cfg(test)]
mod cache_tests {
    use super::*;
    #[async_std::test]
    async fn cancelled_cache_hit_preserves_existing_destination() {
        let temp = tempfile::tempdir().unwrap();
        let destination = temp.path().join("existing.jar");
        fs::write(&destination, b"previous").await.unwrap();
        let hash = format!("{:x}", Sha1::digest(b"cached"));
        fs::write(temp.path().join(&hash), b"cached").await.unwrap();
        let cancel = Cancel::default();
        cancel.cancel();

        assert!(matches!(
            place(
                "https://example.invalid/file",
                &destination,
                Some(&hash),
                temp.path(),
                &cancel
            )
            .await,
            Err(DownloadError::Cancelled)
        ));
        assert_eq!(fs::read(destination).await.unwrap(), b"previous");
    }

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
