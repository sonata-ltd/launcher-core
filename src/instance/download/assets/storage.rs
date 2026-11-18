use std::{
    collections::HashSet,
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
        files,
        object_store::{ObjectStore, StoreAccess},
    },
};

const INSERT_CHUNK: usize = 300;

pub struct DbAssetStore {
    db: Arc<Database>,
    objects: PathBuf,
    cancel: Cancellation,
}

impl DbAssetStore {
    pub fn new(db: Arc<Database>, objects: PathBuf, cancel: Cancellation) -> Self {
        Self {
            db,
            objects,
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
        let objects = self.objects.clone();
        let cancel = self.cancel.clone();
        super::retention::attach(self.db.clone(), objects.clone(), cancel.clone()).await?;
        let root = files::child(&objects, Path::new(".sonata-cas"))
            .map_err(|error| DbError::InvalidInput(error.to_string()))?;
        let access = ObjectStore::new(root)
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
    objects: &Path,
    access: &StoreAccess,
    asset: &AssetInfo,
    cancel: &Cancellation,
) -> io::Result<bool> {
    let relative = PathBuf::from(&asset.hash[..2]).join(&asset.hash);
    let path = files::child(objects, &relative).map_err(io::Error::other)?;
    let present = match std::fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.is_file() => {
            files::matches_cancellable(&path, asset.hash(), cancel)?
        }
        // Never overwrite directories, links or special files with an asset.
        Ok(_) => return Err(io::Error::other("asset placement is not regular")),
        Err(error) if error.kind() == io::ErrorKind::NotFound => false,
        Err(error) => return Err(error),
    };
    if present {
        // Import verifies SHA-1 while copying and creates an isolated SHA-256
        // object. Existing corrupt objects are reported, never clobbered.
        if access.find_sha1(asset.hash(), cancel)?.is_none() {
            let _: crate::utils::object_store::StoredObject =
                access.import_checked(&path, cancel, |digests| {
                    if digests.sha1 != asset.hash {
                        return Err(io::Error::other("asset changed during import"));
                    }
                    Ok(())
                })?;
        }
        return Ok(true);
    }
    let Some(object) = access.find_sha1(asset.hash(), cancel)? else {
        return Ok(false);
    };
    files::placement::copy_replace(&object.path, &path, cancel)?;
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
