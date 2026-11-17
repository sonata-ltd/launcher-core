use std::{collections::HashSet, path::PathBuf, sync::Arc};

use async_trait::async_trait;
use sqlx::{QueryBuilder, Sqlite};

use crate::{
    data::db::{Database, DbError},
    instance::download::{assets::AssetInfo, sync::SyncStore},
    utils::{cancellation::Cancellation, files},
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
}

#[async_trait]
impl SyncStore<AssetInfo> for DbAssetStore {
    async fn cached(&self, wanted: &[AssetInfo]) -> Result<HashSet<String>, DbError> {
        let objects = self.objects.clone();
        let wanted = wanted.to_vec();
        let cancel = self.cancel.clone();
        async_std::task::spawn_blocking(move || {
            let mut found = HashSet::new();
            for asset in wanted {
                if cancel.is_cancelled() {
                    break;
                }
                let path = asset.object_path(&objects);
                let cached = match std::fs::symlink_metadata(&path) {
                    Ok(metadata) if metadata.is_file() => {
                        files::matches_cancellable(&path, asset.hash(), &cancel)
                    }
                    Ok(_) => Ok(false),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
                    Err(error) => Err(error),
                };
                if cached.map_err(|error| {
                    DbError::InvalidInput(format!("{}: {error}", path.display()))
                })? {
                    found.insert(asset.hash().clone());
                }
            }
            Ok(found)
        })
        .await
    }

    async fn register(&self, items: &[AssetInfo]) -> Result<(), DbError> {
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
