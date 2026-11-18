//! SQL идентичностей и leases общего хранилища. Нет HTTP и обхода диска.
use crate::{data::db::Database, instance::content::error::Result};

#[cfg(test)]
use crate::instance::content::store::StoredObject;

#[cfg(test)]
pub(in crate::instance::content) async fn register(
    db: &Database,
    object: &StoredObject,
) -> Result<i64> {
    let id = sqlx::query_scalar(
        "INSERT INTO content_objects(sha256,size_bytes,sha1,sha512,store_state) VALUES(?,?,?,?,'ready')
         ON CONFLICT(sha256) DO UPDATE SET sha1=excluded.sha1,sha512=excluded.sha512,store_state='ready'
         RETURNING id"
    ).bind(&object.digests.sha256).bind(object.digests.size_bytes as i64)
        .bind(&object.digests.sha1).bind(&object.digests.sha512)
        .fetch_one(&db.pool).await?;
    Ok(id)
}

/// Возвращается только идентичность объекта, не произвольный путь из БД.
/// Реальный файл вычисляется ObjectStore и проверяется перед использованием.
pub(in crate::instance::content) async fn cached_hash(
    db: &Database,
    sha1: Option<&str>,
    sha512: Option<&str>,
    size: u64,
    operation: uuid::Uuid,
) -> Result<Option<String>> {
    if sha1.is_none() && sha512.is_none() {
        return Ok(None);
    }
    let mut tx = db.pool.begin().await?;
    let found: Option<(i64, String)> = sqlx::query_as("SELECT id,sha256 FROM content_objects WHERE store_state='ready' AND size_bytes=? AND (? IS NULL OR sha1=?) AND (? IS NULL OR sha512=?) LIMIT 1")
        .bind(size as i64).bind(sha1).bind(sha1).bind(sha512).bind(sha512).fetch_optional(&mut *tx).await?;
    if let Some((object, _)) = &found {
        lease(&mut tx, operation, *object).await?;
    }
    tx.commit().await?;
    Ok(found.map(|(_, hash)| hash))
}

async fn lease(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    operation: uuid::Uuid,
    object: i64,
) -> Result<()> {
    let preparing: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM content_operations WHERE id=? AND state='preparing')",
    )
    .bind(operation.to_string())
    .fetch_one(&mut **tx)
    .await?;
    if !preparing {
        return Err(super::super::error::ContentError::InvalidJournalId(
            operation.to_string(),
        ));
    }
    sqlx::query("INSERT OR IGNORE INTO content_object_leases(operation_id,object_id) VALUES(?,?)")
        .bind(operation.to_string())
        .bind(object)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

pub(in crate::instance::content) async fn register_leased(
    db: &Database,
    hashes: &crate::utils::files::digest::FileDigests,
    ready: bool,
    operation: uuid::Uuid,
) -> Result<i64> {
    let mut tx = db.pool.begin().await?;
    let id = sqlx::query_scalar(
        "INSERT INTO content_objects(sha256,size_bytes,sha1,sha512,store_state) VALUES(?,?,?,?,?)
         ON CONFLICT(sha256) DO UPDATE SET sha1=excluded.sha1,sha512=excluded.sha512,
             store_state=CASE WHEN excluded.store_state='ready' THEN 'ready' ELSE content_objects.store_state END
         RETURNING id",
    ).bind(&hashes.sha256).bind(hashes.size_bytes as i64).bind(&hashes.sha1).bind(&hashes.sha512)
        .bind(if ready { "ready" } else { "external" }).fetch_one(&mut *tx).await?;
    lease(&mut tx, operation, id).await?;
    tx.commit().await?;
    Ok(id)
}
