use std::{
    io,
    path::{Path, PathBuf},
    sync::Arc,
};

#[derive(Clone)]
pub(crate) struct NativeDirectory(Arc<tempfile::TempDir>);

impl NativeDirectory {
    pub(crate) fn create(root: &Path) -> io::Result<Self> {
        std::fs::create_dir_all(root)?;
        if !std::fs::symlink_metadata(root)?.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "natives root is not a directory",
            ));
        }
        let directory = tempfile::Builder::new().prefix("run-").tempdir_in(root)?;
        Ok(Self(Arc::new(directory)))
    }

    pub(crate) fn finish(self, process_exited: bool) {
        if let Ok(directory) = Arc::try_unwrap(self.0) {
            if process_exited {
                let path = directory.path().to_owned();
                if let Err(error) = directory.close() {
                    tracing::warn!(path = %path.display(), %error, "could not remove native libraries");
                }
            } else {
                let path = directory.keep();
                tracing::warn!(path = %path.display(), "kept native libraries because process exit was unconfirmed");
            }
        }
    }
}

impl AsRef<Path> for NativeDirectory {
    fn as_ref(&self) -> &Path {
        self.0.path()
    }
}

pub struct Natives {}

impl Natives {
    pub async fn extract(paths: Vec<PathBuf>, destination: &Path) -> io::Result<()> {
        async_std::fs::create_dir_all(destination).await?;
        for path in paths {
            Self::extract_archive(path, destination.to_owned(), vec!["META-INF/".into()]).await?;
        }
        Ok(())
    }

    pub(super) async fn extract_archive(
        path: PathBuf,
        destination: impl AsRef<Path> + Send + 'static,
        excludes: Vec<String>,
    ) -> io::Result<()> {
        async_std::task::spawn_blocking(move || {
            crate::modloader::archive::extract(&path, destination.as_ref(), &excludes)
                .map_err(io::Error::other)
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[async_std::test]
    async fn corrupt_native_archive_is_returned_to_the_caller() {
        let temp = tempfile::tempdir().unwrap();
        let archive = temp.path().join("natives.jar");
        std::fs::write(&archive, b"not a ZIP archive").unwrap();
        let destination = temp.path().join("extracted");
        let error = Natives::extract(vec![archive], &destination)
            .await
            .unwrap_err();
        assert!(error.get_ref().is_some());
        assert_eq!(std::fs::read_dir(destination).unwrap().count(), 0);
    }
}
