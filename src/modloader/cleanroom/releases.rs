use crate::{
    modloader::error::{ModloaderError, Result},
    utils::download::transfer::{self, Cancel},
    utils::files,
};
use serde::{Deserialize, Serialize};

use crate::{java::policy::JavaPolicy, modloader::repository::Repository};

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

    let root = repo.cache.clone();
    // Lock/read/modify/atomic-write are one owned blocking transaction. Separate
    // Repository objects, aliases and processes share the same file lock.
    async_std::task::spawn_blocking(move || -> Result<()> {
        use std::ffi::OsStr;
        let root = files::directory::Directory::ensure_path(&root)?;
        let lock = root.open_lock(OsStr::new(".cleanroom-registry.lock"))?;
        let _lock = crate::utils::object_store::StoreLock::blocking_exclusive(lock)?;
        let name = OsStr::new("cleanroom-releases.json");
        let mut all: Vec<CleanroomRelease> = match root.open_file(name) {
            Ok(file) => serde_json::from_reader(std::io::BufReader::new(file))?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(error) => return Err(error.into()),
        };
        all.retain(|entry| entry.version != release.version);
        all.push(release);
        root.write_bytes(name, &serde_json::to_vec_pretty(&all)?)?;
        Ok(())
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release(version: String) -> CleanroomRelease {
        CleanroomRelease {
            version,
            minecraft: "1.12.2".into(),
            archive_url: "https://example.invalid/release.zip".into(),
            archive_sha256: "a".repeat(64),
            archive_root: String::new(),
            java: JavaPolicy {
                majors: vec![21],
                allow_newer: false,
            },
        }
    }

    #[async_std::test]
    async fn concurrent_registrations_across_repository_objects_never_lose_entries() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let results = futures::future::join_all((0..32).map(|id| async move {
            let repo = Repository::new(root.into());
            register(&repo, release(format!("release-{id}"))).await
        }))
        .await;
        assert!(results.iter().all(Result::is_ok));
        let entries = get_releases(&Repository::new(root.into())).await.unwrap();
        assert_eq!(entries.len(), 32);
        assert_eq!(
            entries
                .iter()
                .map(|entry| &entry.version)
                .collect::<std::collections::HashSet<_>>()
                .len(),
            32
        );
    }

    #[async_std::test]
    async fn corrupt_registry_is_preserved_and_registration_does_not_replace_unknown_data() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("cleanroom-releases.json");
        std::fs::write(&path, b"corrupt old registry").unwrap();
        assert!(register(
            &Repository::new(directory.path().into()),
            release("new".into())
        )
        .await
        .is_err());
        assert_eq!(std::fs::read(path).unwrap(), b"corrupt old registry");
    }
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
