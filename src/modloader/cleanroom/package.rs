use std::{
    collections::HashMap,
    io,
    path::{Path, PathBuf},
};

use serde::Deserialize;
use serde_json::Value;

use crate::{
    modloader::{
        archive, cleanroom::releases::CleanroomRelease, components, error::ModloaderError,
        error::Result, model::MetadataPin, repository::Repository,
    },
    utils::download::transfer::{self, Cancel},
    utils::{files, object_store::catalog::read_regular},
};

const MAX_PACK_BYTES: u64 = 8 * 1024 * 1024;
const MAX_ALL_PATCH_BYTES: u64 = 64 * 1024 * 1024;

async fn read_pack_json(path: PathBuf, remaining: u64) -> io::Result<Vec<u8>> {
    async_std::task::spawn_blocking(move || read_regular(&path, remaining.min(MAX_PACK_BYTES)))
        .await
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
        serde_json::from_slice(&read_pack_json(root.join("mmc-pack.json"), MAX_PACK_BYTES).await?)?;
    if pack.format_version != 1 || pack.components.len() > 128 {
        return Err(ModloaderError::Unsupported("MMC pack format".into()));
    }

    let mut local = HashMap::new();
    let mut roots = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut patch_bytes_remaining = MAX_ALL_PATCH_BYTES;

    for entry in pack.components {
        cancel.check()?;
        if !seen.insert(entry.uid.clone()) || entry.uid.contains(['/', '\\', ':']) {
            return Err(ModloaderError::Invalid("invalid/duplicate MMC uid".into()));
        }

        let patch = root.join("patches").join(format!("{}.json", entry.uid));

        match read_pack_json(patch, patch_bytes_remaining).await {
            Ok(bytes) => {
                patch_bytes_remaining -= bytes.len() as u64;
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

    archive::stage_embedded_libraries(root.join("libraries"), maven.to_owned(), cancel).await?;

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
