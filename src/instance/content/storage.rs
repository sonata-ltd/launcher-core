//! SQL привязок и журнала; файловые изменения находятся в install/recovery.
use super::journal::{self, Placement, PublishedFile};
use crate::{
    data::db::Database,
    instance::content::error::{ContentError, Result},
    instance::model::InstanceId,
};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, sqlx::Type)]
#[sqlx(rename_all = "snake_case")]
enum OperationState {
    Preparing,
    Publishing,
    Completed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, sqlx::Type)]
#[sqlx(rename_all = "snake_case")]
enum ModPresence {
    Present,
    Missing,
    Unstable,
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
        "SELECT EXISTS(SELECT 1 FROM content_operations WHERE instance_id=? AND state IN (?,?))",
    )
    .bind(instance.get())
    .bind(OperationState::Publishing)
    .bind(OperationState::Preparing)
    .fetch_one(&db.pool)
    .await
}

pub(super) async fn begin_preparation(
    db: &Database,
    id: Uuid,
    instance: InstanceId,
    store_root: &[u8],
) -> Result<()> {
    sqlx::query("INSERT INTO content_operations(id,instance_id,state,recovery_json,store_root) VALUES(?,?,'preparing',?,?)")
        .bind(id.to_string()).bind(instance.get()).bind(journal::encode(&[])?).bind(store_root)
        .execute(&db.pool).await?;
    Ok(())
}

/// Only callable during startup while the corresponding store's root lock is held.
/// Preparing never changes mods; publishing journals must not be discarded.
pub(super) async fn clear_abandoned_preparations(db: &Database, store_root: &[u8]) -> Result<()> {
    sqlx::query("DELETE FROM content_operations WHERE state='preparing' AND store_root=?")
        .bind(store_root)
        .execute(&db.pool)
        .await?;
    Ok(())
}

pub(super) async fn discard_preparation(db: &Database, id: Uuid) -> Result<()> {
    sqlx::query("DELETE FROM content_operations WHERE id=? AND state='preparing'")
        .bind(id.to_string())
        .execute(&db.pool)
        .await?;
    Ok(())
}

pub(super) async fn prepare(
    db: &Database,
    id: Uuid,
    instance: InstanceId,
    files: &[PublishedFile],
) -> Result<()> {
    let mut transaction = db.pool.begin().await?;
    let transition = sqlx::query(
        "INSERT INTO content_operations(id,instance_id,state,recovery_json) VALUES(?,?,?,?)
         ON CONFLICT(id) DO UPDATE SET state=excluded.state,recovery_json=excluded.recovery_json
         WHERE content_operations.instance_id=excluded.instance_id AND content_operations.state='preparing'",
    )
    .bind(id.to_string())
    .bind(instance.get())
    .bind(OperationState::Publishing)
    .bind(journal::encode(files)?)
    .execute(&mut *transaction)
    .await?;
    if transition.rows_affected() != 1 {
        return Err(ContentError::InvalidJournalId(id.to_string()));
    }
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

pub(super) async fn complete(
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
        let evidence = if file.placement == Placement::External {
            "verified_hash"
        } else {
            "download_hash"
        };
        let provider_id: i64 = sqlx::query_scalar(
            "INSERT INTO content_provider_files(object_id,provider,project_id,release_id,file_key,match_evidence,dependencies_json)
             VALUES(?,?,?,?,?,?,?) ON CONFLICT(object_id,provider,project_id,release_id,file_key)
             DO UPDATE SET match_evidence=CASE WHEN content_provider_files.match_evidence='download_hash'
                 THEN content_provider_files.match_evidence ELSE excluded.match_evidence END,
             dependencies_json=COALESCE(excluded.dependencies_json,content_provider_files.dependencies_json)
             RETURNING id"
        ).bind(file.object_id).bind(file.provider).bind(&file.project_id).bind(&file.release_id).bind(&file.name).bind(evidence).bind(file.dependencies.as_ref().map(serde_json::to_string).transpose()?)
            .fetch_one(&mut *transaction).await?;
        sqlx::query("INSERT INTO instance_content(instance_id,object_id,kind,relative_path,path_key,mod_enabled,placement,presence,preferred_provider_file_id)
                     VALUES(?,?,'mod',?,?,1,?,?,?)
                     ON CONFLICT(instance_id,path_key) DO UPDATE SET
                         object_id=excluded.object_id, kind=excluded.kind,
                         relative_path=excluded.relative_path, mod_enabled=excluded.mod_enabled,
                         placement=excluded.placement, presence=excluded.presence,
                         observed_size=NULL, observed_modified=NULL, observed_identity=NULL,
                         indexed_fingerprint=NULL,
                         pinned=CASE WHEN instance_content.object_id=excluded.object_id
                                     THEN instance_content.pinned ELSE 0 END,
                         preferred_provider_file_id=excluded.preferred_provider_file_id")
            .bind(instance.get()).bind(file.object_id).bind(&relative).bind(crate::utils::files::path_key(std::path::Path::new(&relative)))
            .bind(file.placement).bind(ModPresence::Present).bind(provider_id).execute(&mut *transaction).await?;
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

#[derive(sqlx::FromRow)]
pub(super) struct RegisteredMod {
    id: i64,
    pub relative_path: String,
    pub object_sha256: String,
    pub provider: super::model::Provider,
    pub project_id: String,
    pub release_id: String,
    pub size_bytes: i64,
    pub dependencies_json: Option<String>,
}

impl RegisteredMod {
    pub(super) fn project(&self) -> super::model::ProjectKey {
        super::model::ProjectKey {
            provider: self.provider,
            project_id: self.project_id.clone(),
        }
    }

    pub(super) fn dependencies(&self) -> Result<Vec<super::model::Dependency>> {
        self.dependencies_json
            .as_deref()
            .map(serde_json::from_str)
            .transpose()
            .map(|deps| deps.unwrap_or_default())
            .map_err(Into::into)
    }

    pub(super) fn verified_path<'a>(
        &self,
        snapshot: &'a super::inventory::ModSnapshot,
    ) -> Option<&'a std::path::Path> {
        let recorded = std::path::Path::new(&self.relative_path);
        if snapshot
            .archives
            .get(recorded)
            .is_some_and(|hash| hash.eq_ignore_ascii_case(&self.object_sha256))
        {
            return snapshot
                .archives
                .get_key_value(recorded)
                .map(|(path, _)| path.as_path());
        }
        let mut paths = snapshot
            .archives
            .iter()
            .filter(|(_, hash)| hash.eq_ignore_ascii_case(&self.object_sha256))
            .map(|(path, _)| path.as_path());
        let first = paths.next()?;
        paths.next().is_none().then_some(first)
    }

    pub(super) fn matches(
        &self,
        project: &super::model::ProjectKey,
        snapshot: &super::inventory::ModSnapshot,
    ) -> bool {
        self.provider == project.provider
            && self.project_id == project.project_id
            && self.verified_path(snapshot).is_some()
    }
}

pub(super) async fn has_registered_mods(db: &Database, instance: InstanceId) -> Result<bool> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM instance_content WHERE instance_id=? AND kind='mod' AND preferred_provider_file_id IS NOT NULL)",
    )
    .bind(instance.get()).fetch_one(&db.pool).await?)
}

pub(super) async fn registered_mods(
    db: &Database,
    instance: InstanceId,
) -> Result<Vec<RegisteredMod>> {
    Ok(sqlx::query_as(
        "SELECT i.id, i.relative_path, o.sha256 AS object_sha256, p.provider, p.project_id, p.release_id,
                o.size_bytes, p.dependencies_json
         FROM instance_content AS i
         JOIN content_objects AS o ON o.id=i.object_id
         JOIN content_provider_files AS p ON p.id=i.preferred_provider_file_id
          AND p.object_id=i.object_id
         WHERE i.instance_id=? AND i.kind='mod'",
    )
    .bind(instance.get())
    .fetch_all(&db.pool)
    .await?)
}

/// Reconciles registered files against a completed scan while the instance is reserved.
pub(super) async fn reconcile_registered_mods(
    db: &Database,
    instance: InstanceId,
    snapshot: &super::inventory::ModSnapshot,
) -> Result<()> {
    let registered = registered_mods(db, instance).await?;
    let mut transaction = db.pool.begin().await?;
    for entry in registered {
        let recorded = std::path::Path::new(&entry.relative_path);
        let observed = entry.verified_path(snapshot);
        let rename = observed
            .filter(|path| *path != recorded)
            .and_then(|path| path.to_str());
        if let Some(path) = rename {
            let path = std::path::Path::new(path);
            let key = crate::utils::files::path_key(path);
            let changed = sqlx::query(
                "UPDATE instance_content SET relative_path=?, path_key=?, mod_enabled=?, presence=?
                 WHERE id=? AND NOT EXISTS(SELECT 1 FROM instance_content
                   WHERE instance_id=? AND path_key=? AND id<>?)",
            )
            .bind(path.to_str())
            .bind(&key)
            .bind(
                path.extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("jar")),
            )
            .bind(ModPresence::Present)
            .bind(entry.id)
            .bind(instance.get())
            .bind(&key)
            .bind(entry.id)
            .execute(&mut *transaction)
            .await?;
            if changed.rows_affected() == 1 {
                continue;
            }
        }
        let presence = if observed == Some(recorded) {
            ModPresence::Present
        } else if snapshot.observed.contains(recorded) {
            ModPresence::Unstable
        } else {
            ModPresence::Missing
        };
        sqlx::query("UPDATE instance_content SET presence=? WHERE id=? AND presence<>?")
            .bind(presence)
            .bind(entry.id)
            .bind(presence)
            .execute(&mut *transaction)
            .await?;
    }
    transaction.commit().await?;
    Ok(())
}
