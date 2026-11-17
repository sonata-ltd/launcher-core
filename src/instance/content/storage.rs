//! SQL привязок и журнала; файловые изменения находятся в install/recovery.
use super::journal::{self, PublishedFile};
use crate::{
    data::db::Database,
    instance::content::error::{ContentError, Result},
    instance::model::InstanceId,
};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, sqlx::Type)]
#[sqlx(rename_all = "snake_case")]
enum OperationState {
    Publishing,
    Completed,
}

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
        "SELECT EXISTS(SELECT 1 FROM content_operations WHERE instance_id=? AND state=?)",
    )
    .bind(instance.get())
    .bind(OperationState::Publishing)
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
    sqlx::query(
        "INSERT INTO content_operations(id,instance_id,state,recovery_json) VALUES(?,?,?,?)",
    )
    .bind(id.to_string())
    .bind(instance.get())
    .bind(OperationState::Publishing)
    .bind(journal::encode(files)?)
    .execute(&mut *transaction)
    .await?;
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
    let transition = sqlx::query(
        "UPDATE content_operations SET state=?
         WHERE id=? AND instance_id=? AND state=?",
    )
    .bind(OperationState::Completed)
    .bind(id.to_string())
    .bind(instance.get())
    .bind(OperationState::Publishing)
    .execute(&mut *transaction)
    .await?;
    if transition.rows_affected() != 1 {
        return Err(ContentError::InvalidJournalId(id.to_string()));
    }
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
    sqlx::query("DELETE FROM content_object_leases WHERE operation_id=?")
        .bind(id.to_string())
        .execute(&mut *transaction)
        .await?;
    transaction.commit().await?;
    Ok(())
}

#[derive(sqlx::FromRow)]
pub(super) struct PendingPublication {
    pub id: String,
    pub instance_id: InstanceId,
    pub recovery_json: String,
}

pub(super) async fn pending(
    db: &Database,
    instance: Option<InstanceId>,
) -> Result<Vec<PendingPublication>> {
    Ok(sqlx::query_as(
        "SELECT id, instance_id, recovery_json FROM content_operations
         WHERE state=? AND (?2 IS NULL OR instance_id=?2)
         ORDER BY instance_id, created_at, rowid",
    )
    .bind(OperationState::Publishing)
    .bind(instance.map(InstanceId::get))
    .fetch_all(&db.pool)
    .await?)
}

pub(super) async fn leased_objects(
    db: &Database,
    operation: Uuid,
) -> Result<std::collections::HashMap<i64, String>> {
    let rows = sqlx::query_as(
        "SELECT objects.id, objects.sha256 FROM content_object_leases AS leases
         JOIN content_objects AS objects ON objects.id = leases.object_id
         WHERE leases.operation_id = ?",
    )
    .bind(operation.to_string())
    .fetch_all(&db.pool)
    .await?;
    Ok(rows.into_iter().collect())
}

pub(crate) async fn operation_completed(db: &Database, id: Uuid) -> Result<bool> {
    let state: Option<OperationState> =
        sqlx::query_scalar("SELECT state FROM content_operations WHERE id=?")
            .bind(id.to_string())
            .fetch_optional(&db.pool)
            .await?;
    Ok(state == Some(OperationState::Completed))
}

/// Завершённый recovery-журнал больше не защищает файлы. Убираем его,
/// чтобы RESTRICT не запрещал в будущем удалить обычный экземпляр.
pub(crate) async fn clear_completed(db: &Database, id: Option<Uuid>) -> Result<()> {
    if let Some(id) = id {
        sqlx::query("DELETE FROM content_operations WHERE id=? AND state=?")
            .bind(id.to_string())
            .bind(OperationState::Completed)
            .execute(&db.pool)
            .await?;
    } else {
        sqlx::query("DELETE FROM content_operations WHERE state=?")
            .bind(OperationState::Completed)
            .execute(&db.pool)
            .await?;
    }
    Ok(())
}

pub(super) async fn clear_completed_for_instance(
    db: &Database,
    instance: InstanceId,
) -> Result<()> {
    sqlx::query("DELETE FROM content_operations WHERE instance_id=? AND state=?")
        .bind(instance.get())
        .bind(OperationState::Completed)
        .execute(&db.pool)
        .await?;
    Ok(())
}

/// Releases the journal and its leases without changing any published files.
pub(super) async fn abandon(db: &Database, instance: InstanceId, operation: &str) -> Result<()> {
    let deleted =
        sqlx::query("DELETE FROM content_operations WHERE id=? AND instance_id=? AND state=?")
            .bind(operation)
            .bind(instance.get())
            .bind(OperationState::Publishing)
            .execute(&db.pool)
            .await?;
    if deleted.rows_affected() != 1 {
        return Err(ContentError::InvalidJournalId(operation.to_owned()));
    }
    Ok(())
}
