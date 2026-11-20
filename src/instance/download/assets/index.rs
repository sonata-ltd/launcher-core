//! Raw asset index schema shared between installation and retention.
use serde::{
    de::{self, MapAccess, Visitor},
    Deserialize, Deserializer,
};
use std::{collections::BTreeMap, fmt, fs, io::Write, path::Path};

use crate::{
    data::layout::LauncherPaths,
    instance::download::manifest::{asset_index_id, ManifestError},
    utils::{
        download::{download_manifest_bytes, metadata_cache, MAX_MANIFEST_BYTES},
        files,
        object_store::catalog::read_regular,
    },
};
use sha1::{Digest, Sha1};

use super::{parse, AssetInfo, AssetSyncError};

pub(crate) const VERIFIED_INDEX: &str = "_sonata_verified_asset_index";

struct Expected<'a> {
    id: &'a str,
    size: u64,
    sha1: &'a str,
}

impl<'a> Expected<'a> {
    fn from_manifest(manifest: &'a serde_json::Value) -> Result<Self, ManifestError> {
        Ok(Self {
            id: asset_index_id(manifest)?,
            size: manifest["assetIndex"]["size"]
                .as_u64()
                .filter(|size| *size <= MAX_MANIFEST_BYTES as u64)
                .ok_or_else(|| {
                    ManifestError::ManifestMalformed("invalid assetIndex.size".into())
                })?,
            sha1: manifest["assetIndex"]["sha1"]
                .as_str()
                .filter(|hash| files::hash_valid(hash, 40))
                .ok_or_else(|| {
                    ManifestError::ManifestMalformed("invalid assetIndex.sha1".into())
                })?,
        })
    }

    fn verify(&self, body: &[u8]) -> Result<(), ManifestError> {
        let actual_size = body.len() as u64;
        if actual_size != self.size {
            return Err(self.error(format!(
                "expected {} bytes, received {actual_size}",
                self.size
            )));
        }
        let actual = format!("{:x}", Sha1::digest(body));
        if !actual.eq_ignore_ascii_case(self.sha1) {
            return Err(self.error(format!("expected SHA-1 {}, received {actual}", self.sha1)));
        }
        Ok(())
    }

    fn error(&self, details: impl Into<String>) -> ManifestError {
        ManifestError::AssetIndexIntegrity {
            id: self.id.into(),
            details: details.into(),
        }
    }
}

pub(crate) struct VerifiedIndex {
    id: String,
    body: Vec<u8>,
    url: String,
    index: Index,
}

impl VerifiedIndex {
    pub(crate) async fn download(manifest: &serde_json::Value) -> Result<Self, ManifestError> {
        let expected = Expected::from_manifest(manifest)?;
        let url = manifest["assetIndex"]["url"]
            .as_str()
            .ok_or_else(|| ManifestError::ManifestMalformed("missing assetIndex.url".into()))?;
        let body = download_manifest_bytes(url)
            .await
            .map_err(ManifestError::DownloadFailed)?;
        expected.verify(&body)?;
        let index = serde_json::from_slice(&body).map_err(ManifestError::InvalidAssetIndexJson)?;
        Ok(Self {
            id: expected.id.into(),
            body,
            url: url.into(),
            index,
        })
    }

    pub(crate) fn into_parts(self) -> Result<(IndexPublication, Vec<AssetInfo>), AssetSyncError> {
        let objects = self
            .index
            .objects
            .into_iter()
            .map(|(name, object)| parse::asset(name, Some(&object.hash)))
            .collect::<Result<_, _>>()?;
        Ok((
            IndexPublication {
                id: self.id,
                body: self.body,
                url: self.url,
            },
            objects,
        ))
    }
}

pub(crate) struct IndexPublication {
    id: String,
    body: Vec<u8>,
    url: String,
}

impl IndexPublication {
    pub(crate) async fn publish(self, paths: &LauncherPaths) -> Result<(), ManifestError> {
        let cache = files::child(paths.meta(), Path::new("asset-indexes"))?;
        let assets = paths.assets().clone();
        async_std::task::spawn_blocking(move || {
            let path = game_path(&assets, &self.id)?;
            let directory = game_directory(&assets)?;
            fs::create_dir_all(&directory).map_err(files::FileError::from)?;
            let mut staged =
                tempfile::NamedTempFile::new_in(directory).map_err(files::FileError::from)?;
            staged
                .write_all(&self.body)
                .map_err(files::FileError::from)?;
            staged
                .as_file()
                .sync_all()
                .map_err(files::FileError::from)?;
            files::atomic_write(&metadata_cache::entry_path(&cache, &self.url), &self.body)?;
            match staged.persist_noclobber(&path) {
                Ok(_) => Ok(()),
                Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {
                    let published = read_regular(&path, MAX_MANIFEST_BYTES as u64)
                        .map_err(files::FileError::from)?;
                    if published == self.body {
                        Ok(())
                    } else {
                        Err(ManifestError::AssetIndexIntegrity {
                            id: self.id,
                            details: "game index ID already has different bytes".into(),
                        })
                    }
                }
                Err(error) => Err(files::FileError::from(error.error).into()),
            }
        })
        .await
    }
}

fn game_directory(assets: &Path) -> Result<std::path::PathBuf, files::FileError> {
    files::child(assets, Path::new("indexes"))
}

fn game_path(assets: &Path, id: &str) -> Result<std::path::PathBuf, files::FileError> {
    files::child(assets, &Path::new("indexes").join(format!("{id}.json")))
}

pub(crate) async fn verify_published(
    manifest: &serde_json::Value,
    assets: &Path,
) -> Result<(), ManifestError> {
    let expected = Expected::from_manifest(manifest)?;
    let path = game_path(assets, expected.id)?;
    let body =
        async_std::task::spawn_blocking(move || read_regular(&path, MAX_MANIFEST_BYTES as u64))
            .await
            .map_err(files::FileError::from)?;
    expected.verify(&body)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Index {
    #[serde(deserialize_with = "unique_objects")]
    pub(super) objects: BTreeMap<String, Object>,
    #[serde(rename = "virtual", default, deserialize_with = "present_bool")]
    _virtual: Option<bool>,
    #[serde(
        rename = "map_to_resources",
        default,
        deserialize_with = "present_bool"
    )]
    _resources: Option<bool>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Object {
    pub(super) hash: String,
    #[serde(rename = "size", default, deserialize_with = "present_size")]
    _size: Option<u64>,
}

// Omission is historical/optional; an explicitly present null is not a bool/size.
fn present_bool<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<bool>, D::Error> {
    bool::deserialize(deserializer).map(Some)
}

fn present_size<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<u64>, D::Error> {
    u64::deserialize(deserializer).map(Some)
}

fn unique_objects<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<BTreeMap<String, Object>, D::Error> {
    struct Objects;
    impl<'de> Visitor<'de> for Objects {
        type Value = BTreeMap<String, Object>;

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            formatter.write_str("an asset object map without duplicate names")
        }

        fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
            let mut objects = BTreeMap::new();
            while let Some(name) = map.next_key::<String>()? {
                if objects.insert(name, map.next_value()?).is_some() {
                    return Err(de::Error::custom("duplicate asset object name"));
                }
            }
            Ok(objects)
        }
    }
    deserializer.deserialize_map(Objects)
}
