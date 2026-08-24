use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use async_trait::async_trait;
use sqlx::{QueryBuilder, Sqlite};

use crate::{
    data::db::{Database, DbError},
    instance::{
        download::{libs::LibInfo, sync::SyncStore},
        model::InstanceId,
    },
};

const HASH_CHUNK: usize = 500;
const INSERT_CHUNK: usize = 150;

pub struct DbLibStore {
    db: Arc<Database>,
}

impl DbLibStore {
    pub fn new(db: Arc<Database>) -> Self {
        Self { db }
    }

    pub async fn link_instance(&self, id: InstanceId, libs: &[&LibInfo]) -> Result<(), DbError> {
        let mut tx = self.db.pool.begin().await?;

        sqlx::query!("DELETE FROM instances_libraries WHERE instance_id = ?", id)
            .execute(&mut *tx)
            .await?;

        let mut position: i64 = 0;

        for chunk in libs.chunks(INSERT_CHUNK) {
            let mut lookup: QueryBuilder<Sqlite> =
                QueryBuilder::new("SELECT id, hash FROM libraries WHERE hash IN (");

            let mut list = lookup.separated(", ");

            for lib in chunk {
                list.push_bind(lib.hash().clone());
            }

            lookup.push(")");

            let found: Vec<(i64, String)> = lookup.build_query_as().fetch_all(&mut *tx).await?;
            let ids: HashMap<String, i64> = found
                .into_iter()
                .map(|(lib_id, hash)| (hash, lib_id))
                .collect();

            let mut rows: Vec<(i64, i64)> = Vec::with_capacity(chunk.len());

            for lib in chunk {
                let Some(library_id) = ids.get(lib.hash()) else {
                    continue;
                };

                rows.push((*library_id, position));
                position += 1;
            }

            if rows.is_empty() {
                continue;
            }

            let mut insert: QueryBuilder<Sqlite> = QueryBuilder::new(
                "INSERT INTO instances_libraries (instance_id, library_id, position) ",
            );

            insert.push_values(&rows, |mut row, (library_id, at)| {
                row.push_bind(id).push_bind(*library_id).push_bind(*at);
            });

            insert.push(
                " ON CONFLICT(instance_id, library_id) DO UPDATE SET position = excluded.position",
            );

            insert.build().execute(&mut *tx).await?;
        }

        tx.commit().await?;

        Ok(())
    }

    pub async fn for_instance(&self, id: InstanceId) -> Result<Vec<LibInfo>, DbError> {
        let libs = sqlx::query_as!(
            LibInfo,
            r#"
                SELECT
                    l.hash   AS "hash!: String",
                    l.name   AS "name!: String",
                    l.path   AS "path!: String",
                    l.url    AS "url!: String",
                    l.native AS "native!: bool"
                FROM instances_libraries il
                JOIN libraries l ON l.id = il.library_id
                WHERE il.instance_id = ?
                ORDER BY il.position
            "#,
            id
        )
        .fetch_all(&self.db.pool)
        .await?;

        Ok(libs)
    }
}

#[async_trait]
impl SyncStore<LibInfo> for DbLibStore {
    async fn cached(&self, wanted: &[LibInfo]) -> Result<HashSet<String>, DbError> {
        let mut found = HashSet::with_capacity(wanted.len());

        for chunk in wanted.chunks(HASH_CHUNK) {
            let mut qb: QueryBuilder<Sqlite> =
                QueryBuilder::new("SELECT hash FROM libraries WHERE hash IN (");

            let mut list = qb.separated(", ");

            for lib in chunk {
                list.push_bind(lib.hash().clone());
            }

            qb.push(")");

            let hashes: Vec<String> = qb.build_query_scalar().fetch_all(&self.db.pool).await?;
            found.extend(hashes);
        }

        Ok(found)
    }

    async fn register(&self, items: &[LibInfo]) -> Result<(), DbError> {
        for chunk in items.chunks(INSERT_CHUNK) {
            let mut qb: QueryBuilder<Sqlite> =
                QueryBuilder::new("INSERT INTO libraries (name, hash, path, native, url) ");

            qb.push_values(chunk, |mut row, lib| {
                row.push_bind(lib.name().clone())
                    .push_bind(lib.hash().clone())
                    .push_bind(lib.path().clone())
                    .push_bind(lib.is_native())
                    .push_bind(lib.url().clone());
            });

            qb.push(" ON CONFLICT(hash) DO NOTHING");

            qb.build().execute(&self.db.pool).await?;
        }

        Ok(())
    }
}
