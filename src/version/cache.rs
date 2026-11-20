use crate::utils::{files::directory::Directory, object_store::catalog::read_open_regular};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::{
    io,
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
        let relative = path
            .strip_prefix(&root)
            .map_err(|error| VersionError::Cache(error.to_string()))?;
        let file = match root
            .canonicalize()
            .and_then(|root| Directory::open(&root))
            .and_then(|root| root.open_entry_file(relative))
        {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(VersionError::Cache(format!("{}: {error}", path.display()))),
        };
        let bytes = read_open_regular(&file, MAX_CACHE_BYTES as u64)
            .map_err(|error| VersionError::Cache(error.to_string()))?;
        let index: DiskIndex = serde_json::from_slice(&bytes)
            .map_err(|error| VersionError::Cache(error.to_string()))?;
        index.validate(provider)?;
        Ok(Some(index))
    })
    .await
}

pub(super) async fn write(root: PathBuf, provider: MetaProvider, index: DiskIndex) -> Result<()> {
    async_std::task::spawn_blocking(move || {
        let store = || -> io::Result<()> {
            std::fs::create_dir_all(&root)?;
            let directory = Directory::open(&root.canonicalize()?)?;
            let provider = provider.to_string().to_ascii_lowercase();
            let directory = directory.create_relative(Path::new(&provider))?;
            let name = std::ffi::OsStr::new("net.minecraft.json");
            directory.with_staged_file(
                |file| {
                    serde_json::to_writer(&mut *file, &index).map_err(io::Error::other)?;
                    file.sync_all()
                },
                |staged, ()| {
                    directory.require_regular_or_missing(name)?;
                    directory.replace(staged, name)
                },
            )
        };
        store().map_err(|error| VersionError::Cache(error.to_string()))
    })
    .await
}
