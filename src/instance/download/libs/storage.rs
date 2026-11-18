use std::{
    collections::HashSet,
    path::{Path, PathBuf},
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
    utils::{download::Downloadable, files},
};

const INSERT_CHUNK: usize = 100;

pub struct DbLibStore {
    db: Arc<Database>,
    library_root: Option<PathBuf>,
}

impl DbLibStore {
    pub fn new(db: Arc<Database>) -> Self {
        Self {
            db,
            library_root: None,
        }
    }

    pub fn at(db: Arc<Database>, library_root: &Path) -> Self {
        Self {
            db,
            library_root: Some(library_root.to_owned()),
        }
    }

    pub async fn link_instance(&self, id: InstanceId, libs: &[&LibInfo]) -> Result<(), DbError> {
        let paths: Vec<String> = libs
            .iter()
            .map(|lib| {
                let Some(root) = &self.library_root else {
                    return Ok(lib.path().clone());
                };
                let path = Path::new(lib.path());
                let relative = if path.is_absolute() {
                    path.strip_prefix(root).map_err(|_| {
                        DbError::InvalidInput("library placement outside configured root".into())
                    })?
                } else {
                    path
                };
                if relative.to_string_lossy().contains('\\') {
                    return Err(DbError::ResultCorrupted);
                }
                files::child(root, relative)
                    .map_err(|error| DbError::InvalidInput(error.to_string()))?;
                relative
                    .to_str()
                    .map(str::to_owned)
                    .ok_or(DbError::ResultCorrupted)
            })
            .collect::<Result<_, _>>()?;
        let mut tx = self.db.pool.begin().await?;
        sqlx::query("DELETE FROM instance_library_placements WHERE instance_id = ?")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        for (chunk_index, chunk) in libs.chunks(INSERT_CHUNK).enumerate() {
            let mut insert: QueryBuilder<Sqlite> = QueryBuilder::new(
                "INSERT INTO instance_library_placements(instance_id,position,hash,name,path,native,url) ",
            );
            insert.push_values(chunk.iter().enumerate(), |mut row, (index, lib)| {
                row.push_bind(id)
                    .push_bind((chunk_index * INSERT_CHUNK + index) as i64)
                    .push_bind(lib.hash())
                    .push_bind(lib.name())
                    .push_bind(&paths[chunk_index * INSERT_CHUNK + index])
                    .push_bind(lib.is_native())
                    .push_bind(lib.url());
            });
            insert.build().execute(&mut *tx).await?;
        }
        tx.commit().await?;
        Ok(())
    }

    pub async fn for_instance(&self, id: InstanceId) -> Result<Vec<LibInfo>, DbError> {
        Ok(sqlx::query_as(
            "SELECT hash,name,path,url,native FROM instance_library_placements
             WHERE instance_id=? ORDER BY position",
        )
        .bind(id)
        .fetch_all(&self.db.pool)
        .await?)
    }
}

#[async_trait]
impl SyncStore<LibInfo> for DbLibStore {
    async fn cached(&self, wanted: &[LibInfo]) -> Result<HashSet<String>, DbError> {
        let wanted = wanted.to_vec();
        async_std::task::spawn_blocking(move || {
            let mut found = HashSet::new();
            for lib in wanted {
                if files::matches(std::path::Path::new(lib.path()), lib.hash())
                    .map_err(|e| DbError::InvalidInput(e.to_string()))?
                {
                    found.insert(lib.cache_key());
                }
            }
            Ok(found)
        })
        .await
    }

    // A verified byte sequence alone has no semantic placement to register.
    // The ordered instance snapshot is committed by link_instance after sync.
    async fn register(&self, _: &[LibInfo]) -> Result<(), DbError> {
        Ok(())
    }
}
