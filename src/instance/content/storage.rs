//! SQL привязок и журнала; файловые изменения находятся в install/recovery.
use super::api::PublishedFile;
use crate::{
    data::db::Database,
    instance::content::error::{ContentError, Result},
    instance::model::InstanceId,
};
use sqlx::Row;
use uuid::Uuid;

pub async fn ensure_no_pending(db: &Database, instance: InstanceId) -> Result<()> {
    if has_pending(db, instance).await? {
        return Err(ContentError::Invalid(
            "Осталась незавершённая установка контента: требуется восстановление".into(),
        ));
    }
    Ok(())
}

pub(crate) async fn has_pending(
    db: &Database,
    instance: InstanceId,
) -> std::result::Result<bool, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM content_operations WHERE instance_id=? AND state='publishing')",
    )
    .bind(instance.get())
    .fetch_one(&db.pool)
    .await
}

pub(crate) async fn prepare(
    db: &Database,
    id: Uuid,
    instance: InstanceId,
    files: &[PublishedFile],
) -> Result<()> {
    let mut transaction = db.pool.begin().await?;
    sqlx::query("INSERT INTO content_operations(id,instance_id,state,plan_json,recovery_json) VALUES(?,?,'publishing','{}',?)")
        .bind(id.to_string()).bind(instance.get()).bind(serde_json::to_string(files)?)
        .execute(&mut *transaction).await?;
    for file in files {
        sqlx::query(
            "INSERT OR IGNORE INTO content_object_leases(operation_id,object_id) VALUES(?,?)",
        )
        .bind(id.to_string())
        .bind(file.object_id)
        .execute(&mut *transaction)
        .await?;
    }
    transaction.commit().await?;
    Ok(())
}

pub(crate) async fn complete(
    db: &Database,
    id: Uuid,
    instance: InstanceId,
    files: &[PublishedFile],
) -> Result<()> {
    let mut transaction = db.pool.begin().await?;
    for file in files {
        let relative = format!("mods/{}", file.name);
        let provider_id: i64 = sqlx::query_scalar(
            "INSERT INTO content_provider_files(object_id,provider,project_id,release_id,file_key,match_evidence)
             VALUES(?,?,?,?,?,'download_hash') ON CONFLICT(object_id,provider,project_id,release_id,file_key)
             DO UPDATE SET match_evidence='download_hash' RETURNING id"
        ).bind(file.object_id).bind(&file.provider).bind(&file.project_id).bind(&file.release_id).bind(&file.name)
            .fetch_one(&mut *transaction).await?;
        sqlx::query("INSERT INTO instance_content(instance_id,object_id,kind,relative_path,path_key,mod_enabled,placement,preferred_provider_file_id)
                     VALUES(?,?,'mod',?,?,1,?,?)")
            .bind(instance.get()).bind(file.object_id).bind(&relative).bind(crate::utils::files::path_key(std::path::Path::new(&relative)))
            .bind(&file.placement).bind(provider_id).execute(&mut *transaction).await?;
    }
    sqlx::query("UPDATE content_operations SET state='completed' WHERE id=?")
        .bind(id.to_string())
        .execute(&mut *transaction)
        .await?;
    sqlx::query("DELETE FROM content_object_leases WHERE operation_id=?")
        .bind(id.to_string())
        .execute(&mut *transaction)
        .await?;
    transaction.commit().await?;
    Ok(())
}

pub(crate) async fn pending(
    db: &Database,
) -> Result<Vec<(String, InstanceId, Vec<PublishedFile>)>> {
    let mut operations = Vec::new();
    for row in sqlx::query(
        "SELECT id,instance_id,recovery_json FROM content_operations WHERE state='publishing'",
    )
    .fetch_all(&db.pool)
    .await?
    {
        let instance: i64 = row.try_get("instance_id")?;
        let json: String = row.try_get("recovery_json")?;
        operations.push((
            row.try_get("id")?,
            instance.into(),
            serde_json::from_str(&json)?,
        ));
    }
    Ok(operations)
}

pub(crate) async fn operation_completed(db: &Database, id: Uuid) -> Result<bool> {
    let state: Option<String> =
        sqlx::query_scalar("SELECT state FROM content_operations WHERE id=?")
            .bind(id.to_string())
            .fetch_optional(&db.pool)
            .await?;
    Ok(state.as_deref() == Some("completed"))
}

/// Завершённый recovery-журнал больше не защищает файлы. Убираем его,
/// чтобы RESTRICT не запрещал в будущем удалить обычный экземпляр.
pub(crate) async fn clear_completed(db: &Database, id: Option<Uuid>) -> Result<()> {
    if let Some(id) = id {
        sqlx::query("DELETE FROM content_operations WHERE id=? AND state='completed'")
            .bind(id.to_string())
            .execute(&db.pool)
            .await?;
    } else {
        sqlx::query("DELETE FROM content_operations WHERE state='completed'")
            .execute(&db.pool)
            .await?;
    }
    Ok(())
}
