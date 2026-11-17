use std::{collections::HashSet, sync::Arc};

use async_trait::async_trait;
use sqlx::{QueryBuilder, Sqlite};

use crate::{
    data::db::{Database, DbError},
    instance::download::{assets::AssetInfo, sync::SyncStore},
};

const HASH_CHUNK: usize = 500;
const INSERT_CHUNK: usize = 300;

pub struct DbAssetStore {
    db: Arc<Database>,
}

impl DbAssetStore {
    pub fn new(db: Arc<Database>) -> Self {
        Self { db }
    }
}

#[async_trait]
impl SyncStore<AssetInfo> for DbAssetStore {
    async fn cached(&self, wanted: &[AssetInfo]) -> Result<HashSet<String>, DbError> {
        let mut found = HashSet::with_capacity(wanted.len());

        for chunk in wanted.chunks(HASH_CHUNK) {
            let mut qb: QueryBuilder<Sqlite> =
                QueryBuilder::new("SELECT hash FROM assets WHERE hash IN (");

            let mut list = qb.separated(", ");

            for asset in chunk {
                list.push_bind(asset.hash().clone());
            }

            qb.push(")");

            let hashes: Vec<String> = qb.build_query_scalar().fetch_all(&self.db.pool).await?;
            found.extend(hashes);
        }

        Ok(found)
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
