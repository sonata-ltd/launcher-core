use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use serde::Deserialize;
use serde_json::Value;

use crate::{
    instance::download::transfer::{self, Cancel},
    modloader::{
        archive, cleanroom::releases::CleanroomRelease, components, error::ModloaderError,
        error::Result, model::MetadataPin, repository::Repository,
    },
    utils::files,
};

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
    result.pins.push(MetadataPin {
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
