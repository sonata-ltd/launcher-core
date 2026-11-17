//! Только SQL объектов общего хранилища. Нет экземпляров, HTTP и обхода диска.
use crate::{
    data::db::Database,
    instance::content::{error::Result, store::StoredObject},
};

pub async fn register(db: &Database, object: &StoredObject) -> Result<i64> {
    let id = sqlx::query_scalar(
        "INSERT INTO content_objects(sha256,size_bytes,sha1,sha512,store_path,store_state) VALUES(?,?,?,?,?,'ready')
         ON CONFLICT(sha256) DO UPDATE SET store_path=excluded.store_path,store_state='ready'
         RETURNING id"
    ).bind(&object.digests.sha256).bind(object.digests.size_bytes as i64)
        .bind(&object.digests.sha1).bind(&object.digests.sha512).bind(object.path.to_string_lossy().as_ref())
        .fetch_one(&db.pool).await?;
    Ok(id)
}

/// Возвращается только идентичность объекта, не произвольный путь из БД.
/// Реальный файл вычисляется ObjectStore и проверяется перед использованием.
pub async fn cached_hash(
    db: &Database,
    sha1: Option<&str>,
    sha512: Option<&str>,
    size: u64,
) -> Result<Option<String>> {
    if sha1.is_none() && sha512.is_none() {
        return Ok(None);
    }
    Ok(sqlx::query_scalar("SELECT sha256 FROM content_objects WHERE store_state='ready' AND size_bytes=? AND (? IS NULL OR sha1=?) AND (? IS NULL OR sha512=?) LIMIT 1")
        .bind(size as i64).bind(sha1).bind(sha1).bind(sha512).bind(sha512).fetch_optional(&db.pool).await?)
}
