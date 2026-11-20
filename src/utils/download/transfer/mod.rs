pub mod error;
use super::{
    destination::{Destination, Target},
    error::DownloadError as StreamError,
    stream::{self, Length, Stream},
};
use crate::{
    utils::download::transfer::error::{DownloadError, Result},
    utils::files,
    utils::object_store::ObjectStore,
};
#[cfg(test)]
use async_std::fs;
use async_std::task;
#[cfg(test)]
use sha1::Digest;
use sha1::Sha1;
use std::{
    ffi::OsStr,
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::Arc,
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
    download_in(url, destination.to_owned().into(), expected, cancel).await
}

pub(crate) async fn download_in(
    url: &str,
    destination: Destination,
    expected: Option<&str>,
    cancel: &Cancel,
) -> Result<String> {
    validate_hash(expected)?;
    cancel.check()?;
    let target = Arc::new(task::spawn_blocking(move || destination.bind()).await?);
    download_to(url, target, expected, cancel).await
}

fn validate_hash(expected: Option<&str>) -> Result<()> {
    if let Some(hash) = expected {
        if !files::hash_valid(hash, 40) {
            return Err(DownloadError::Invalid("invalid SHA-1".into()));
        }
    }
    Ok(())
}

#[cfg(all(test, any(unix, windows)))]
#[async_std::test]
async fn download_declines_a_cached_file_in_a_redirected_directory() {
    let scope = tempfile::tempdir().unwrap();
    let parent = scope.path().join("libraries");
    let other = scope.path().join("other");
    std::fs::create_dir(&other).unwrap();
    std::fs::write(other.join("library.jar"), b"keep these bytes").unwrap();
    let digest = files::sha1_file(&other.join("library.jar")).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&other, &parent).unwrap();
    #[cfg(windows)]
    assert!(std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(&parent)
        .arg(&other)
        .status()
        .unwrap()
        .success());

    let result = download(
        "unused-url",
        &parent.join("library.jar"),
        Some(&digest),
        &Cancel::new(),
    )
    .await;

    assert!(matches!(result, Err(DownloadError::Io(_))));
    assert_eq!(
        std::fs::read(other.join("library.jar")).unwrap(),
        b"keep these bytes"
    );
}

async fn download_to(
    url: &str,
    target: Arc<Target>,
    expected: Option<&str>,
    cancel: &Cancel,
) -> Result<String> {
    cancel.check()?;
    if let Some(hash) = expected {
        let expected = hash.to_owned();
        let token = cancel.clone();
        let check = target.clone();
        let cached = task::spawn_blocking(move || check.matches(&expected, &token)).await;
        cancel.check()?;
        if cached? {
            return Ok(hash.to_owned());
        }
    }

    const MAX_ATTEMPTS: u32 = 3;
    for attempt in 1..=MAX_ATTEMPTS {
        cancel.check()?;

        let result = async {
            const MAX_ARTIFACT_BYTES: u64 = 2 * 1024 * 1024 * 1024;
            let (mut response, final_url) =
                stream::response(url, false, Some("SonataLauncher/0.1"), cancel)
                    .await
                    .map_err(map_stream_error)?;
            let directory = target.clone();
            let (temporary, mut output) = task::spawn_blocking(move || directory.stage()).await?;
            let hash = Stream {
                url: final_url.as_str(),
                destination: &temporary.path,
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
                    url: super::http::diagnostic_url(url),
                    expected: expected.into(),
                    actual: hash,
                });
            }

            output.sync_all().await?;
            drop(output);
            cancel.check()?;

            let destination = target.clone();
            let token = cancel.clone();
            task::spawn_blocking(move || -> Result<()> {
                token.check()?;
                temporary.publish(&destination.name)?;
                Ok(())
            })
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
    place_in(url, path.to_owned().into(), expected, blobs, cancel).await
}

pub(crate) async fn place_in(
    url: &str,
    destination: Destination,
    expected: Option<&str>,
    blobs: &Path,
    cancel: &Cancel,
) -> Result<String> {
    validate_hash(expected)?;
    cancel.check()?;
    let access = ObjectStore::new(blobs.to_owned()).access(cancel).await;
    cancel.check()?;
    let access = access?;
    let target = Arc::new(task::spawn_blocking(move || destination.bind()).await?);
    let index = files::sha256(url.as_bytes());
    let read_index = index.clone();
    let reader = access.clone();
    let cached = task::spawn_blocking(move || -> io::Result<String> {
        let urls = reader.directory().open_relative(Path::new("urls"))?;
        let mut hash = String::new();
        urls.open_file(OsStr::new(&read_index))?
            .take(41)
            .read_to_string(&mut hash)?;
        Ok(hash)
    })
    .await
    .ok()
    .filter(|value| files::hash_valid(value, 40));

    let expected = expected.or(cached.as_deref());

    cancel.check()?;
    if let Some(hash) = expected {
        if !files::hash_valid(hash, 40) {
            return Err(DownloadError::Invalid("invalid SHA-1".into()));
        }
        let legacy_blob = blobs.join(hash.to_ascii_lowercase());
        let store = access.clone();
        let target = target.clone();
        let digest = hash.to_owned();
        let cancellation = cancel.clone();
        let placed = task::spawn_blocking(move || -> Result<bool> {
            let object = if let Some(object) = store.find_sha1(&digest, &cancellation)? {
                object
            } else {
                if !files::matches_cancellable(&legacy_blob, &digest, &cancellation)? {
                    return Ok(false);
                }
                // Read-compatible migration; keep the legacy file for older clients.
                store.import_checked(&legacy_blob, &cancellation, |hashes| {
                    if !hashes.sha1.eq_ignore_ascii_case(&digest) {
                        return Err(DownloadError::Invalid(
                            "legacy blob changed during import".into(),
                        ));
                    }
                    Ok(())
                })?
            };
            files::placement::copy_replace_in(
                &object.path,
                &target.directory,
                &target.name,
                &cancellation,
            )?;
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

    let hash = download_to(url, target.clone(), expected, cancel).await?;

    cache_blob(&target.path(), access.clone(), &hash, cancel).await?;

    let body = hash.as_bytes().to_vec();
    let token = cancel.clone();
    task::spawn_blocking(move || -> Result<()> {
        token.check()?;
        let urls = access.directory().create_relative(Path::new("urls"))?;
        let name = OsStr::new(&index);
        urls.with_staged_file(
            |output| {
                output.write_all(&body)?;
                output.sync_all()
            },
            |staged, ()| {
                urls.require_regular_or_missing(name)?;
                urls.replace(staged, name)
            },
        )?;
        Ok(())
    })
    .await?;

    Ok(hash)
}

async fn cache_blob(
    source: &Path,
    store: crate::utils::object_store::StoreAccess,
    expected: &str,
    cancel: &Cancel,
) -> Result<()> {
    let source = source.to_owned();
    let expected = expected.to_owned();
    let cancel = cancel.clone();
    task::spawn_blocking(move || -> Result<()> {
        store.import_checked(&source, &cancel, |hashes| {
            if !hashes.sha1.eq_ignore_ascii_case(&expected) {
                return Err(DownloadError::Invalid(
                    "source changed before cache import".into(),
                ));
            }
            Ok(())
        })?;
        Ok(())
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
    async fn cancelled_maintenance_wait_does_not_read_the_cache_or_touch_destination() {
        use futures::{pin_mut, poll};
        let temp = tempfile::tempdir().unwrap();
        let blobs = temp.path().join("blobs");
        let store = ObjectStore::new(blobs.clone());
        let maintenance = store.try_maintenance().unwrap();
        let destination = temp.path().join("previous.jar");
        fs::write(&destination, b"previous bytes").await.unwrap();
        let cancel = Cancel::new();
        let place = place("not-a-network-url", &destination, None, &blobs, &cancel);
        pin_mut!(place);
        assert!(poll!(place.as_mut()).is_pending());
        cancel.cancel();
        assert!(place.await.unwrap_err().is_cancelled());
        assert_eq!(fs::read(&destination).await.unwrap(), b"previous bytes");
        assert!(!blobs.join("staging").exists());
        assert!(!blobs.join("urls").exists());
        drop(maintenance);
        assert!(store.try_access().is_ok());
    }

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
    async fn corrupt_legacy_blob_is_bypassed_and_shared_store_needs_no_network() {
        let temp = tempfile::tempdir().unwrap();
        let blobs = temp.path().join("blobs");
        fs::create_dir_all(&blobs).await.unwrap();
        let source = temp.path().join("verified.jar");
        fs::write(&source, b"verified content").await.unwrap();
        let hash = files::sha1_file(&source).unwrap();
        let blob = blobs.join(&hash);
        fs::write(&blob, b"corrupt").await.unwrap();
        let access = ObjectStore::new(blobs.clone()).try_access().unwrap();
        cache_blob(&source, access, &hash, &Cancel::default())
            .await
            .unwrap();
        assert_eq!(fs::read(&blob).await.unwrap(), b"corrupt");
        let object = ObjectStore::new(blobs.clone())
            .find_sha1(&hash, &Cancel::default())
            .unwrap()
            .unwrap();
        assert!(files::matches(&object.path, &hash).unwrap());
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
