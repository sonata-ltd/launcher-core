//! SQL привязок и журнала; файловые изменения находятся в install/recovery.
use super::journal::{self, Placement, PublishedFile};
use crate::{
    data::db::Database,
    instance::content::error::{ContentError, Result},
    instance::model::InstanceId,
};
use std::{
    collections::{hash_map::Entry, HashMap},
    path::Path,
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
        return Err(ContentError::RecoveryPending(instance));
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

/// Initiated by startup under service ownership; its cleanup task retains
/// exclusive CAS maintenance until SQL finishes, even if bootstrap is dropped.
/// Preparing never changes mods; publishing journals must not be discarded.
pub(super) async fn clear_abandoned_preparations(
    db: &Database,
    maintenance: &crate::utils::object_store::StoreMaintenance,
) -> Result<()> {
    sqlx::query("DELETE FROM content_operations WHERE state='preparing' AND store_root=?")
        .bind(maintenance.root_key())
        .execute(&db.pool)
        .await?;
    Ok(())
}

#[cfg(test)]
pub(super) async fn preparation_checkpoint(db: &Database) -> Result<Option<Uuid>> {
    let id: Option<String> = sqlx::query_scalar(
        "SELECT id FROM content_operations WHERE state='preparing' ORDER BY rowid LIMIT 1",
    )
    .fetch_optional(&db.pool)
    .await?;
    Ok(id.and_then(|id| Uuid::parse_str(&id).ok()))
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
    let existing = binding_paths(&mut transaction, instance).await?;
    for file in files {
        let relative = format!("mods/{}", file.name);
        let key = existing_binding_key(&existing, Path::new(&relative), None)?;
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
            .bind(instance.get()).bind(file.object_id).bind(&relative).bind(key)
            .bind(file.placement).bind(ModPresence::Present).bind(provider_id).execute(&mut *transaction).await?;
    }
    sqlx::query("DELETE FROM content_object_leases WHERE operation_id=?")
        .bind(id.to_string())
        .execute(&mut *transaction)
        .await?;
    transaction.commit().await?;
    Ok(())
}

pub(super) type BindingPath = (i64, String, Vec<u8>);

pub(super) async fn binding_paths(
    connection: &mut sqlx::SqliteConnection,
    instance: InstanceId,
) -> Result<Vec<BindingPath>> {
    Ok(
        sqlx::query_as(
            "SELECT id,relative_path,path_key FROM instance_content WHERE instance_id=?",
        )
        .bind(instance.get())
        .fetch_all(connection)
        .await?,
    )
}

pub(super) fn existing_binding_key(
    existing: &[BindingPath],
    path: &Path,
    exclude: Option<i64>,
) -> Result<Vec<u8>> {
    let new = crate::utils::files::path_key(path);
    let mut key = None;
    for (id, relative, saved_key) in existing {
        if Some(*id) == exclude {
            continue;
        }
        let same_path = Path::new(relative) == path;
        if !same_path && saved_key != &new {
            continue;
        }
        if !same_path || exclude.is_some() || key.replace(saved_key.clone()).is_some() {
            return Err(ContentError::AmbiguousBinding {
                path: path.to_owned(),
            });
        }
    }
    Ok(key.unwrap_or(new))
}

/// Existing placements must use the exact name enumerated by the filesystem.
/// Path lookup alone can resolve multiple spellings to one directory entry.
pub(super) fn require_observed_name(path: &Path) -> Result<()> {
    let directory = path
        .parent()
        .ok_or_else(|| ContentError::AmbiguousBinding {
            path: path.to_owned(),
        })?;
    let Some(name) = path.file_name() else {
        return Err(ContentError::AmbiguousBinding {
            path: path.to_owned(),
        });
    };
    for entry in std::fs::read_dir(directory)? {
        if entry?.file_name() == name {
            return Ok(());
        }
    }
    Err(ContentError::AmbiguousBinding {
        path: path.to_owned(),
    })
}

pub(super) async fn guard_binding_aliases(
    db: &Database,
    instance: InstanceId,
    root: &Path,
    relative: &Path,
    exclude: Option<i64>,
) -> Result<()> {
    use crate::utils::files::{self, identity::FileIdentity};
    let mut connection = db.pool.acquire().await?;
    let rows = binding_paths(&mut connection, instance).await?;
    existing_binding_key(&rows, relative, exclude)?;
    let target = files::child(root, relative)?;
    let target_id = match FileIdentity::at(&target) {
        Ok(identity) => identity,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    require_observed_name(&target)?;
    let Some(target_parent) = target.parent() else {
        return Err(ContentError::AmbiguousBinding { path: target });
    };
    let parent_id = FileIdentity::at(target_parent)?;
    for (id, other, _) in rows {
        let other = Path::new(&other);
        if Some(id) == exclude || other == relative {
            continue;
        }
        let other = files::child(root, other)?;
        let other_id = match FileIdentity::at(&other) {
            Ok(identity) => identity,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
        };
        if target_id != other_id {
            continue;
        }
        let Some(other_parent) = other.parent() else {
            return Err(ContentError::AmbiguousBinding { path: target });
        };
        if parent_id == FileIdentity::at(other_parent)?
            && (other.file_name() == target.file_name() || require_observed_name(&other).is_err())
        {
            return Err(ContentError::AmbiguousBinding { path: target });
        }
    }
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

#[derive(sqlx::FromRow, serde::Serialize)]
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
        let json = self.dependencies_json.as_deref().ok_or_else(|| {
            ContentError::MissingDependencyMetadata(super::model::ReleaseKey {
                project: self.project(),
                release_id: self.release_id.clone(),
            })
        })?;
        Ok(serde_json::from_str(json)?)
    }
}

pub(super) fn registered_signature(registered: &[RegisteredMod]) -> Result<String> {
    Ok(crate::utils::files::sha256(&serde_json::to_vec(
        registered,
    )?))
}

/// An observed archive contributes evidence to at most one historical row.
/// Exact matches reserve their path before any relocation claim is considered.
pub(super) fn verified_paths<'a>(
    registered: &[RegisteredMod],
    snapshot: &'a super::inventory::ModSnapshot,
) -> Vec<Option<&'a Path>> {
    let mut verified = vec![None; registered.len()];
    let mut exact = HashMap::<&Path, Option<usize>>::new();
    let mut unresolved = Vec::new();
    for (index, row) in registered.iter().enumerate() {
        let Some((path, _)) = snapshot
            .archives
            .get_key_value(Path::new(&row.relative_path))
            .filter(|(_, hash)| hash.eq_ignore_ascii_case(&row.object_sha256))
        else {
            unresolved.push(index);
            continue;
        };
        let path = path.as_path();
        match exact.entry(path) {
            Entry::Vacant(entry) => {
                entry.insert(Some(index));
                verified[index] = Some(path);
            }
            Entry::Occupied(mut entry) => {
                if let Some(other) = entry.get_mut().take() {
                    verified[other] = None;
                }
            }
        }
    }
    if unresolved.is_empty() {
        return verified;
    }
    let mut by_digest = HashMap::<&str, Option<&Path>>::new();
    for (path, digest) in &snapshot.archives {
        by_digest
            .entry(digest)
            .and_modify(|match_| *match_ = None)
            .or_insert(Some(path));
    }
    let mut relocated = HashMap::<&Path, Option<usize>>::new();
    for index in unresolved {
        let digest = registered[index].object_sha256.to_ascii_lowercase();
        let Some(Some(path)) = by_digest.get(digest.as_str()).copied() else {
            continue;
        };
        if !exact.contains_key(path) {
            relocated
                .entry(path)
                .and_modify(|claim| *claim = None)
                .or_insert(Some(index));
        }
    }
    for (path, claim) in relocated {
        if let Some(index) = claim {
            verified[index] = Some(path);
        }
    }
    verified
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
         WHERE i.instance_id=? AND i.kind='mod' ORDER BY i.id",
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
    let verified = verified_paths(&registered, snapshot);
    let mut transaction = db.pool.begin().await?;
    let mut existing = binding_paths(&mut transaction, instance).await?;
    for (entry, observed) in registered.into_iter().zip(verified) {
        let recorded = std::path::Path::new(&entry.relative_path);
        let rename = observed
            .filter(|path| *path != recorded)
            .and_then(|path| path.to_str());
        let relocation = rename.and_then(|name| {
            let path = Path::new(name);
            existing_binding_key(&existing, path, Some(entry.id))
                .ok()
                .map(|key| (path, key))
        });
        if let Some((path, key)) = relocation {
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
                if let Some(row) = existing.iter_mut().find(|row| row.0 == entry.id) {
                    row.1 = path.to_string_lossy().into_owned();
                    row.2 = key;
                }
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
