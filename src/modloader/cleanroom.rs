use super::{
    archive, components,
    error::{ModloaderError, Result},
    files,
    model::JavaPolicy,
    repository::Repository,
    transfer::{self, Cancel},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CleanroomRelease {
    pub version: String,
    pub minecraft: String,
    pub archive_url: String,
    pub archive_sha256: String,
    pub archive_root: String,
    pub java: JavaPolicy,
}

#[derive(Deserialize)]
struct Pack {
    #[serde(rename = "formatVersion")]
    format_version: u32,
    components: Vec<Entry>,
}

#[derive(Deserialize)]
struct Entry {
    uid: String,
    version: String,
}

pub async fn releases(repo: &Repository) -> Result<Vec<CleanroomRelease>> {
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

    let mut all = releases(repo).await?;
    all.retain(|v| v.version != release.version);
    all.push(release);

    let bytes = serde_json::to_vec_pretty(&all)?;
    let path = repo.cache.join("cleanroom-releases.json");

    async_std::task::spawn_blocking(move || files::atomic_write(&path, &bytes)).await
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

pub async fn prepare(
    repo: &Repository,
    release: &CleanroomRelease,
    temp: &Path,
    maven: &Path,
    cancel: &Cancel,
) -> Result<components::Components> {
    let zip = temp.join("cleanroom.zip");

    transfer::download(&release.archive_url, &zip, None, cancel).await?;

    let expected = release.archive_sha256.clone();
    let input = zip.clone();
    let unpack = temp.join("mmc");
    let target = unpack.clone();

    async_std::task::spawn_blocking(move || {
        if files::sha256_file(&input)? != expected {
            return Err(ModloaderError::Invalid(
                "Cleanroom archive checksum mismatch".into(),
            ));
        }

        archive::extract(&input, &target, &[])
    })
    .await?;

    let root = if release.archive_root.is_empty() {
        unpack
    } else {
        files::child(&unpack, Path::new(&release.archive_root))?
    };

    let pack: Pack =
        serde_json::from_slice(&async_std::fs::read(root.join("mmc-pack.json")).await?)?;
    if pack.format_version != 1 || pack.components.len() > 128 {
        return Err(ModloaderError::Unsupported("MMC pack format".into()));
    }

    let mut local = HashMap::new();
    let mut roots = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for entry in pack.components {
        if !seen.insert(entry.uid.clone()) || entry.uid.contains(['/', '\\', ':']) {
            return Err(ModloaderError::Invalid("invalid/duplicate MMC uid".into()));
        }

        let patch = root.join("patches").join(format!("{}.json", entry.uid));

        match async_std::fs::read(patch).await {
            Ok(bytes) => {
                local.insert(entry.uid.clone(), serde_json::from_slice::<Value>(&bytes)?);
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }

        roots.push((entry.uid, entry.version));
    }

    if !roots
        .iter()
        .any(|(uid, v)| uid == "net.minecraft" && v == "1.12.2")
    {
        return Err(ModloaderError::Invalid(
            "Cleanroom pack must target 1.12.2".into(),
        ));
    }

    let source = root.join("libraries");
    let target = maven.to_owned();

    if source.is_dir() {
        async_std::task::spawn_blocking(move || copy_local(source, target)).await?;
    }

    let mut result = components::prism(
        repo,
        "1.12.2",
        "https://meta.prismlauncher.org/v1/",
        local,
        roots,
    )
    .await?;

    let declared = components::policy(&result.manifest)?;
    if !declared
        .majors
        .iter()
        .any(|major| release.java.accepts(*major))
    {
        return Err(ModloaderError::Invalid(
            "descriptor conflicts with Cleanroom Java metadata".into(),
        ));
    }

    result.manifest["compatibleJavaMajors"] = serde_json::json!(release.java.majors);
    result.pins.push(super::model::MetadataPin {
        url: release.archive_url.clone(),
        sha256: release.archive_sha256.clone(),
    });

    Ok(result)
}

fn copy_local(source: PathBuf, target: PathBuf) -> Result<()> {
    let mut todo = vec![(source, target)];

    while let Some((source, target)) = todo.pop() {
        std::fs::create_dir_all(&target)?;

        for entry in std::fs::read_dir(source)? {
            let entry = entry?;
            let kind = entry.file_type()?;

            if kind.is_symlink() {
                return Err(ModloaderError::Invalid("MMC libraries symlink".into()));
            }

            let destination = target.join(entry.file_name());

            if kind.is_dir() {
                todo.push((entry.path(), destination));
            } else {
                std::fs::copy(entry.path(), destination)?;
            }
        }
    }

    Ok(())
}
