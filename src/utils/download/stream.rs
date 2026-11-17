//! Streaming HTTP shared by artifact, asset and managed Java downloads.
use super::{
    error::{DownloadError, Result},
    MAX_REDIRECT_COUNT, READ_TIMEOUT, REQUEST_TIMEOUT,
};
use crate::utils::cancellation::Cancellation;
use async_std::future::timeout;
use futures::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use sha1::Digest;
use std::path::Path;
use surf::{Response, Url};

pub(crate) async fn response(
    url: &str,
    https_only: bool,
    user_agent: Option<&str>,
    cancel: &Cancellation,
) -> Result<(Response, Url)> {
    let mut current = Url::parse(url).map_err(|source| DownloadError::InvalidUrl {
        url: url.to_owned(),
        source,
    })?;

    for redirects in 0..=MAX_REDIRECT_COUNT {
        check_scheme(&current, https_only)?;

        let mut request = surf::get(current.clone()).header("Accept-Encoding", "identity");
        if let Some(user_agent) = user_agent {
            request = request.header("User-Agent", user_agent);
        }

        let response = cancel
            .run(timeout(REQUEST_TIMEOUT, request))
            .await?
            .map_err(|_| DownloadError::Timeout {
                url: current.to_string(),
                phase: "response",
                after: REQUEST_TIMEOUT,
            })?
            .map_err(|error| DownloadError::Request {
                url: current.to_string(),
                source: error.into(),
            })?;

        let status = response.status();
        if status.is_success() {
            return Ok((response, current));
        }
        if !status.is_redirection() {
            return Err(DownloadError::Http {
                status,
                url: current.to_string(),
            });
        }
        if redirects == MAX_REDIRECT_COUNT {
            return Err(DownloadError::TooManyRedirects {
                url: current.to_string(),
                count: redirects,
            });
        }

        current = redirect_target(&current, &response)?;
    }
    unreachable!("redirect loop always returns at the configured maximum")
}

fn check_scheme(url: &Url, https_only: bool) -> Result<()> {
    match url.scheme() {
        "https" => Ok(()),
        "http" if !https_only => Ok(()),
        _ => Err(DownloadError::UnsupportedScheme {
            url: url.to_string(),
            https_only,
        }),
    }
}

fn redirect_target(current: &Url, response: &Response) -> Result<Url> {
    let location = response
        .header("Location")
        .map(|value| value.last().as_str())
        .ok_or_else(|| DownloadError::RedirectWithoutLocation {
            url: current.to_string(),
            status: response.status(),
        })?;
    current
        .join(location)
        .map_err(|source| DownloadError::RedirectTargetInvalid {
            base: current.to_string(),
            location: location.to_owned(),
            source,
        })
}

#[derive(Clone, Copy)]
pub(crate) enum Length {
    UpTo(u64),
    Exact(u64),
}

pub(crate) struct Stream<'a> {
    pub url: &'a str,
    pub destination: &'a Path,
    pub length: Length,
    pub cancel: &'a Cancellation,
}

impl Stream<'_> {
    pub async fn copy<D: Digest>(
        &self,
        reader: &mut (impl AsyncRead + Unpin),
        writer: &mut (impl AsyncWrite + Unpin),
        buffer: &mut [u8],
        mut progress: impl FnMut(u64),
    ) -> Result<String>
    where
        sha1::digest::Output<D>: std::fmt::LowerHex,
    {
        let mut digest = D::new();
        let mut written = 0u64;
        loop {
            let count = self
                .cancel
                .run(timeout(READ_TIMEOUT, reader.read(buffer)))
                .await?
                .map_err(|_| DownloadError::Timeout {
                    url: self.url.to_owned(),
                    phase: "body chunk",
                    after: READ_TIMEOUT,
                })?
                .map_err(|source| DownloadError::BodyRead {
                    url: self.url.to_owned(),
                    source,
                })?;
            if count == 0 {
                break;
            }

            written = written.saturating_add(count as u64);
            self.length.check(self.url, written, false)?;
            self.cancel
                .run(writer.write_all(&buffer[..count]))
                .await?
                .map_err(|source| DownloadError::FileWrite {
                    path: self.destination.to_owned(),
                    source,
                })?;
            digest.update(&buffer[..count]);
            progress(written);
        }

        self.length.check(self.url, written, true)?;
        self.cancel
            .run(writer.flush())
            .await?
            .map_err(|source| DownloadError::FileWrite {
                path: self.destination.to_owned(),
                source,
            })?;
        Ok(format!("{:x}", digest.finalize()))
    }
}

impl Length {
    fn check(self, url: &str, actual: u64, complete: bool) -> Result<()> {
        match self {
            Self::UpTo(limit) if actual > limit => Err(DownloadError::SizeLimit {
                url: url.to_owned(),
                limit,
            }),
            Self::Exact(expected) if actual > expected || (complete && actual != expected) => {
                Err(DownloadError::SizeMismatch { expected, actual })
            }
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_relative_redirects_and_rejects_downgrade_for_https_only_downloads() {
        let base = Url::parse("https://example.invalid/dir/file").unwrap();
        let mut response = http_types::Response::new(surf::StatusCode::TemporaryRedirect);
        response.insert_header("Location", "../other");
        assert_eq!(
            redirect_target(&base, &response.into()).unwrap().as_str(),
            "https://example.invalid/other"
        );

        let mut response = http_types::Response::new(surf::StatusCode::TemporaryRedirect);
        response.insert_header("Location", "http://example.invalid/other");
        let next = redirect_target(&base, &response.into()).unwrap();
        assert!(matches!(
            check_scheme(&next, true),
            Err(DownloadError::UnsupportedScheme { .. })
        ));
        assert!(check_scheme(&next, false).is_ok());
    }

    #[async_std::test]
    async fn oversize_chunk_is_rejected_before_write_or_progress() {
        use futures::io::Cursor;
        let policy = Stream {
            url: "https://example.invalid",
            destination: Path::new("unused"),
            length: Length::UpTo(2),
            cancel: &Cancellation::new(),
        };
        let mut writer = Cursor::new(Vec::new());
        let result = policy
            .copy::<sha1::Sha1>(&mut Cursor::new(b"abc"), &mut writer, &mut [0; 4], |_| {
                panic!("invalid chunk must not report progress")
            })
            .await;
        assert!(matches!(
            result,
            Err(DownloadError::SizeLimit { limit: 2, .. })
        ));
        assert!(writer.into_inner().is_empty());
    }
}
