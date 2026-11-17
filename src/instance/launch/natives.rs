use std::{
    io,
    path::{Path, PathBuf},
};

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
        destination: PathBuf,
        excludes: Vec<String>,
    ) -> io::Result<()> {
        async_std::task::spawn_blocking(move || {
            crate::modloader::archive::extract(&path, &destination, &excludes)
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
