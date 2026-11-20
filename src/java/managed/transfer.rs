use async_std::fs::File;
use sha2::Sha256;
use std::ffi::OsStr;
#[cfg(test)]
use std::path::Path;

use super::model::*;
use crate::utils::{
    cancellation::Cancellation,
    download::{
        error::DownloadError,
        stream::{self, Length, Stream},
    },
    files::directory::Directory,
};

pub(super) async fn download(
    package: &ManagedJavaPackage,
    directory: &Directory,
    cancel: &Cancellation,
    progress: impl Fn(u64),
) -> Result<String> {
    let (mut response, url) = stream::response(&package.url, true, None, cancel)
        .await
        .map_err(download_error)?;
    let name = OsStr::new("archive.part");
    let path = directory.path().join(name);
    let mut file = File::from(
        directory
            .create_private_file(name)
            .map_err(|error| ManagedJavaError::caused_by(ManagedJavaErrorKind::Download, error))?,
    );
    copy_body(
        &mut response,
        &mut file,
        Stream {
            url: url.as_str(),
            destination: &path,
            length: Length::Exact(package.size),
            cancel,
        },
        progress,
    )
    .await
}

#[cfg(test)]
pub(super) async fn stream_body(
    reader: &mut (impl futures::AsyncRead + Unpin),
    writer: &mut (impl futures::AsyncWrite + Unpin),
    expected_size: u64,
    cancel: &Cancellation,
    progress: impl Fn(u64),
) -> Result<String> {
    copy_body(
        reader,
        writer,
        Stream {
            url: "https://example.invalid",
            destination: Path::new("runtime archive"),
            length: Length::Exact(expected_size),
            cancel,
        },
        progress,
    )
    .await
}

async fn copy_body(
    reader: &mut (impl futures::AsyncRead + Unpin),
    writer: &mut (impl futures::AsyncWrite + Unpin),
    stream: Stream<'_>,
    progress: impl Fn(u64),
) -> Result<String> {
    stream
        .copy::<Sha256>(reader, writer, &mut vec![0; 64 * 1024], progress)
        .await
        .map_err(download_error)
}

fn download_error(error: DownloadError) -> ManagedJavaError {
    let kind = if matches!(error, DownloadError::Cancelled(_)) {
        ManagedJavaErrorKind::Cancelled
    } else {
        ManagedJavaErrorKind::Download
    };
    ManagedJavaError::caused_by(kind, error)
}
