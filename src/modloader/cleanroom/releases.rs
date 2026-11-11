use crate::{
    instance::download::transfer::{self, Cancel},
    modloader::error::{ModloaderError, Result},
    utils::files,
};
use serde::{Deserialize, Serialize};

use crate::modloader::{model::JavaPolicy, repository::Repository};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CleanroomRelease {
    pub version: String,
    pub minecraft: String,
    pub archive_url: String,
    pub archive_sha256: String,
    pub archive_root: String,
    pub java: JavaPolicy,
}

pub async fn get_releases(repo: &Repository) -> Result<Vec<CleanroomRelease>> {
    let path = repo.cache.join("cleanroom-releases.json");

    match async_std::fs::read(path).await {
        Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e.into()),
    }
}

pub async fn register(repo: &Repository, release: CleanroomRelease) -> Result<()> {
    if release.minecraft != "1.12.2"
        || !files::hash_valid(&release.archive_sha256, 64)
        || release.java.majors.is_empty()
    {
        return Err(ModloaderError::Invalid(
            "invalid Cleanroom release descriptor".into(),
        ));
    }

    let mut all = get_releases(repo).await?;
    all.retain(|v| v.version != release.version);
    all.push(release);

    let bytes = serde_json::to_vec_pretty(&all)?;
    let path = repo.cache.join("cleanroom-releases.json");

    async_std::task::spawn_blocking(move || files::atomic_write(&path, &bytes)).await?;

    Ok(())
}

pub async fn import_archive(
    repo: &Repository,
    version: String,
    url: String,
    majors: Vec<u32>,
) -> Result<()> {
    let temp = tempfile::tempdir()?;
    let path = temp.path().join("release.zip");

    transfer::download(&url, &path, None, &Cancel::default()).await?;

    let (archive_sha256, archive_root) = async_std::task::spawn_blocking(move || {
        let digest = files::sha256_file(&path)?;
        let zip = zip::ZipArchive::new(std::fs::File::open(path)?)
            .map_err(|e| ModloaderError::Invalid(e.to_string()))?;

        let roots: Vec<_> = zip
            .file_names()
            .filter_map(|name| {
                if name == "mmc-pack.json" {
                    Some(String::new())
                } else {
                    name.strip_suffix("/mmc-pack.json").map(str::to_owned)
                }
            })
            .collect();

        if roots.len() != 1 {
            return Err(ModloaderError::Invalid(
                "archive has no unique mmc-pack.json".into(),
            ));
        }

        Ok::<_, ModloaderError>((digest, roots[0].clone()))
    })
    .await?;

    register(
        repo,
        CleanroomRelease {
            version,
            minecraft: "1.12.2".into(),
            archive_url: url,
            archive_sha256,
            archive_root,
            java: JavaPolicy {
                majors,
                allow_newer: false,
            },
        },
    )
    .await
}
