use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
};

use super::{
    error::{Result, VersionError},
    provider::MetaProvider,
    transport::MAX_INDEX_BYTES,
};

const SCHEMA: u32 = 1;
const COMPONENT: &str = "net.minecraft";
const MAX_CACHE_BYTES: usize = MAX_INDEX_BYTES * 6 + 4096;

#[derive(Debug, Serialize, Deserialize)]
pub(super) struct DiskIndex {
    schema: u32,
    provider: String,
    component: String,
    pub fetched_at: DateTime<Utc>,
    pub raw: String,
}

impl DiskIndex {
    pub fn new(provider: MetaProvider, fetched_at: DateTime<Utc>, raw: String) -> Self {
        Self {
            schema: SCHEMA,
            provider: provider.to_string().to_ascii_lowercase(),
            component: COMPONENT.into(),
            fetched_at,
            raw,
        }
    }

    fn validate(&self, provider: MetaProvider) -> Result<()> {
        if self.schema != SCHEMA
            || self.provider != provider.to_string().to_ascii_lowercase()
            || self.component != COMPONENT
        {
            return Err(VersionError::Cache(
                "incompatible cache identity or schema".into(),
            ));
        }
        if self.raw.len() > MAX_INDEX_BYTES {
            return Err(VersionError::IndexTooLarge(MAX_INDEX_BYTES));
        }
        Ok(())
    }
}

pub(super) fn file_path(root: &Path, provider: MetaProvider) -> PathBuf {
    root.join(provider.to_string().to_ascii_lowercase())
        .join("net.minecraft.json")
}

pub(super) async fn read(root: PathBuf, provider: MetaProvider) -> Result<Option<DiskIndex>> {
    async_std::task::spawn_blocking(move || {
        let path = file_path(&root, provider);
        let file = match std::fs::File::open(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(VersionError::Cache(format!("{}: {error}", path.display()))),
        };
        let mut bytes = Vec::new();
        file.take((MAX_CACHE_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|error| VersionError::Cache(error.to_string()))?;
        if bytes.len() > MAX_CACHE_BYTES {
            return Err(VersionError::Cache("cache file is too large".into()));
        }
        let index: DiskIndex = serde_json::from_slice(&bytes)
            .map_err(|error| VersionError::Cache(error.to_string()))?;
        index.validate(provider)?;
        Ok(Some(index))
    })
    .await
}

pub(super) async fn write(root: PathBuf, provider: MetaProvider, index: DiskIndex) -> Result<()> {
    async_std::task::spawn_blocking(move || {
        let path = file_path(&root, provider);
        let parent = path
            .parent()
            .ok_or_else(|| VersionError::Cache("invalid cache path".into()))?;
        std::fs::create_dir_all(parent).map_err(|error| VersionError::Cache(error.to_string()))?;
        let mut temporary = tempfile::NamedTempFile::new_in(parent)
            .map_err(|error| VersionError::Cache(error.to_string()))?;
        serde_json::to_writer(temporary.as_file_mut(), &index)
            .map_err(|error| VersionError::Cache(error.to_string()))?;
        temporary
            .as_file_mut()
            .flush()
            .map_err(|error| VersionError::Cache(error.to_string()))?;
        temporary
            .as_file()
            .sync_all()
            .map_err(|error| VersionError::Cache(error.to_string()))?;
        temporary
            .persist(&path)
            .map_err(|error| VersionError::Cache(error.to_string()))?;
        Ok(())
    })
    .await
}
