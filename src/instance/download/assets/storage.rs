use std::{
    collections::HashSet,
    ffi::OsStr,
    io,
    path::{Path, PathBuf},
    sync::Arc,
};

use async_trait::async_trait;
use sqlx::{QueryBuilder, Sqlite};

use crate::{
    data::db::{Database, DbError},
    instance::download::{assets::AssetInfo, sync::SyncStore},
    utils::{
        cancellation::Cancellation,
        files::{self, directory::Directory},
        object_store::{ObjectStore, StoreAccess},
    },
};

const INSERT_CHUNK: usize = 300;

pub struct DbAssetStore {
    db: Arc<Database>,
    objects: PathBuf,
    directory: Option<Arc<Directory>>,
    cancel: Cancellation,
}

impl DbAssetStore {
    #[cfg(test)]
    pub fn new(db: Arc<Database>, objects: PathBuf, cancel: Cancellation) -> Self {
        Self {
            db,
            objects,
            directory: None,
            cancel,
        }
    }

    pub(super) fn at(db: Arc<Database>, directory: Arc<Directory>, cancel: Cancellation) -> Self {
        Self {
            db,
            objects: directory.path().to_owned(),
            directory: Some(directory),
            cancel,
        }
    }

    /// Minecraft's SHA-1 layout is a placement, not the authoritative byte store.
    /// Collection requires the complete owner/SQL/index/placement roots gate,
    /// never SQL rows or aliases alone. Callers do not retain a raw CAS path.
    async fn bytes(
        &self,
        items: Vec<AssetInfo>,
        require_all: bool,
    ) -> Result<HashSet<String>, DbError> {
        let objects = match &self.directory {
            Some(directory) => directory.clone(),
            None => {
                let objects = self.objects.clone();
                async_std::task::spawn_blocking(move || Directory::ensure_path(&objects))
                    .await
                    .map_err(bytes_error)?
            }
        };
        let cancel = self.cancel.clone();
        super::retention::attach(self.db.clone(), &objects, cancel.clone()).await?;
        let root = objects
            .open_relative(Path::new(".sonata-cas"))
            .map_err(bytes_error)?;
        let access = ObjectStore::new(root.path().to_owned())
            .access(&cancel)
            .await
            .map_err(bytes_error)?;
        async_std::task::spawn_blocking(move || {
            let mut found = HashSet::new();
            #[cfg(test)]
            tests::checkpoint(access.root(), "admitted");
            for asset in items {
                cancel.check().map_err(io::Error::from)?;
                if materialize(&objects, &access, &asset, &cancel)? {
                    found.insert(asset.hash().clone());
                    #[cfg(test)]
                    tests::checkpoint(access.root(), "imported");
                } else if require_all {
                    return Err(io::Error::other(format!(
                        "downloaded asset {} is missing or corrupt",
                        asset.hash()
                    )));
                }
            }
            Ok(found)
        })
        .await
        .map_err(bytes_error)
    }
}

pub(super) fn bytes_error(error: io::Error) -> DbError {
    DbError::InvalidInput(format!("asset byte store: {error}"))
}

fn materialize(
    objects: &Arc<Directory>,
    access: &StoreAccess,
    asset: &AssetInfo,
    cancel: &Cancellation,
) -> io::Result<bool> {
    let shard_name = &asset.hash[..2];
    let name = OsStr::new(&asset.hash);
    let shard = match objects.open_relative(Path::new(shard_name)) {
        Ok(directory) => Some(directory),
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(error),
    };
    if let Some(shard) = &shard {
        // Import verifies SHA-1 while copying and creates an isolated SHA-256
        // object. Existing corrupt objects are reported, never clobbered.
        if files::matches_open_file(shard.open_file(name), asset.hash(), cancel)? {
            if access.find_sha1(asset.hash(), cancel)?.is_none() {
                let path = shard.path().join(name);
                access.import_checked(&path, cancel, |digests| {
                    if digests.sha1 != asset.hash {
                        return Err(io::Error::other("asset changed during import"));
                    }
                    Ok(())
                })?;
            }
            return Ok(true);
        }
    }
    let Some(object) = access.find_sha1(asset.hash(), cancel)? else {
        return Ok(false);
    };
    let shard = match shard {
        Some(shard) => shard,
        None => objects.create_child(OsStr::new(shard_name))?,
    };
    files::placement::copy_replace_in(&object.path, &shard, name, cancel)?;
    Ok(true)
}

#[cfg(test)]
mod tests;

#[async_trait]
impl SyncStore<AssetInfo> for DbAssetStore {
    async fn cached(&self, wanted: &[AssetInfo]) -> Result<HashSet<String>, DbError> {
        self.bytes(wanted.to_vec(), false).await
    }

    async fn register(&self, items: &[AssetInfo]) -> Result<(), DbError> {
        self.bytes(items.to_vec(), true).await?;
        for chunk in items.chunks(INSERT_CHUNK) {
            let mut qb: QueryBuilder<Sqlite> =
                QueryBuilder::new("INSERT INTO assets (name, hash, url) ");

            qb.push_values(chunk, |mut row, asset| {
                row.push_bind(asset.name().clone())
                    .push_bind(asset.hash().clone())
                    .push_bind(asset.url().clone());
            });

            qb.push(" ON CONFLICT(hash) DO NOTHING");

            qb.build().execute(&self.db.pool).await?;
        }

        Ok(())
    }
}
