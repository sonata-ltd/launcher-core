//! Fail-closed content-CAS collection. Runtime generations use a separate gate.
//! All catalogue databases stay write-locked from root snapshot through unlink
//! and metadata completion; admission excludes upgraded readers/importers.
use super::{
    error::{ContentError, Result},
    journal,
};
use crate::{
    data::db::{retention::LockedDatabases, Database},
    utils::{
        cancellation::Cancellation,
        files,
        object_store::{
            catalog::{read_regular, DatabaseOwner, RootCatalog},
            ObjectStore, StoreMaintenance,
        },
    },
};
use serde::{Deserialize, Serialize};
#[cfg(test)]
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{Sqlite, Transaction};
use std::{
    collections::{HashMap, HashSet},
    fs, io,
    path::Path,
    sync::Arc,
    time::Duration,
};

const INTENTS: &str = ".sonata-gc";

#[cfg(test)]
mod tests;

pub(super) async fn owner(db: &Database) -> Result<Option<DatabaseOwner>> {
    Ok(db.retention_owner().await?)
}

pub(super) async fn attach(db: &Database, maintenance: &StoreMaintenance) -> Result<()> {
    // An owner we cannot describe must poison completeness before new imports.
    let owner = match owner(db).await {
        Ok(owner) => owner,
        Err(error) => {
            RootCatalog::attach(maintenance, None)?;
            tracing::warn!(%error, "unknown database owner disables content collection");
            return Ok(());
        }
    };
    RootCatalog::attach(maintenance, owner)?;
    Ok(())
}

async fn roots(tx: &mut Transaction<'_, Sqlite>, store: &ObjectStore) -> Result<HashSet<String>> {
    // Even a relocated/foreign preparation key may still own unknown hashes.
    let preparing: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM content_operations WHERE state='preparing')",
    )
    .fetch_one(&mut **tx)
    .await?;
    if preparing {
        return Err(ContentError::Invalid(
            "durable content preparation blocks collection".into(),
        ));
    }
    let objects: Vec<(i64, String)> = sqlx::query_as("SELECT id,sha256 FROM content_objects")
        .fetch_all(&mut **tx)
        .await?;
    let mut identities = HashMap::new();
    for (id, hash) in objects {
        store.path_for(&hash)?;
        identities.insert(id, hash.to_ascii_lowercase());
    }
    let references: Vec<i64> = sqlx::query_scalar("SELECT object_id FROM instance_content WHERE object_id IS NOT NULL UNION SELECT object_id FROM content_object_leases")
        .fetch_all(&mut **tx).await?;
    let mut roots = HashSet::new();
    for id in references {
        roots.insert(
            identities
                .get(&id)
                .ok_or_else(|| {
                    ContentError::Invalid("dangling content retention reference".into())
                })?
                .clone(),
        );
    }
    let publications: Vec<(String, String)> =
        sqlx::query_as("SELECT id,recovery_json FROM content_operations WHERE state='publishing'")
            .fetch_all(&mut **tx)
            .await?;
    for (operation, body) in publications {
        let id = uuid::Uuid::parse_str(&operation)
            .map_err(|_| ContentError::InvalidJournalId(operation.clone()))?;
        if id.to_string() != operation {
            return Err(ContentError::InvalidJournalId(operation));
        }
        let leases: Vec<i64> =
            sqlx::query_scalar("SELECT object_id FROM content_object_leases WHERE operation_id=?")
                .bind(&operation)
                .fetch_all(&mut **tx)
                .await?;
        let leases: HashSet<_> = leases.into_iter().collect();
        let mut names = HashSet::new();
        for file in journal::decode(&body)? {
            files::portable_name::validate_file_name(&file.name)?;
            store.path_for(&file.sha256)?;
            if !names.insert(file.name.to_lowercase())
                || !leases.contains(&file.object_id)
                || identities.get(&file.object_id) != Some(&file.sha256.to_ascii_lowercase())
            {
                return Err(ContentError::Invalid(
                    "invalid retention publication entry".into(),
                ));
            }
            roots.insert(file.sha256.to_ascii_lowercase());
        }
    }
    Ok(roots)
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DeleteIntent {
    version: u32,
    sha256: String,
    sha1: String,
    size: u64,
}

impl DeleteIntent {
    fn validate(&self) -> io::Result<()> {
        if self.version != 1
            || !files::hash_valid(&self.sha256, 64)
            || !files::hash_valid(&self.sha1, 40)
            || self.sha256 != self.sha256.to_ascii_lowercase()
            || self.sha1 != self.sha1.to_ascii_lowercase()
        {
            return Err(io::Error::other("invalid content deletion intent"));
        }
        Ok(())
    }
}

/// The whole gate is owned, not only its blocking workers: dropping a caller
/// must not roll back SQL locks while an unlink worker still runs.
pub(super) async fn collect(store: Arc<ObjectStore>, minimum_age: Duration) -> Result<usize> {
    async_std::task::spawn(collect_owned(store, minimum_age)).await
}

async fn collect_owned(store: Arc<ObjectStore>, minimum_age: Duration) -> Result<usize> {
    let maintenance = Arc::new(store.try_maintenance()?);
    let mut databases = LockedDatabases::open(maintenance.clone()).await?;
    let mut retained = HashSet::new();
    for tx in &mut databases.transactions {
        retained.extend(roots(tx, &store).await?);
    }
    #[cfg(test)]
    tests::checkpoint(maintenance.root(), "snapshot").await;
    let worker = maintenance.clone();
    let plan =
        async_std::task::spawn_blocking(move || plan(&worker, &retained, minimum_age)).await?;
    let mut removed = 0;
    for intent in &plan {
        let worker = maintenance.clone();
        let deletion = intent.clone();
        let was_removed =
            async_std::task::spawn_blocking(move || unlink(&worker, &deletion)).await?;
        removed += usize::from(was_removed);
        for tx in &mut databases.transactions {
            // Keep provenance/identity; absence is not a verified cache hit.
            sqlx::query("UPDATE content_objects SET store_state='external' WHERE lower(sha256)=?")
                .bind(&intent.sha256)
                .execute(&mut **tx)
                .await?;
        }
    }
    #[cfg(test)]
    tests::checkpoint(maintenance.root(), "sql").await;
    for (index, tx) in std::mem::take(&mut databases.transactions)
        .into_iter()
        .enumerate()
    {
        tx.commit().await?;
        #[cfg(test)]
        if index == 0 {
            tests::checkpoint(maintenance.root(), "first-commit").await;
        }
        #[cfg(not(test))]
        let _ = index;
    }
    // Intent cleanup follows every database commit; a crash repeats the gate.
    let worker = maintenance.clone();
    async_std::task::spawn_blocking(move || clear_finished_intents(&worker, &plan)).await?;
    #[cfg(test)]
    tests::checkpoint(maintenance.root(), "cleaned").await;
    databases.close().await;
    Ok(removed)
}

fn regular_directory(path: &Path) -> io::Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() => Ok(true),
        Ok(_) => Err(io::Error::other(
            "retention directory is a link or special file",
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

fn plan(
    maintenance: &StoreMaintenance,
    retained: &HashSet<String>,
    age: Duration,
) -> io::Result<Vec<DeleteIntent>> {
    let root = maintenance.root();
    let store = ObjectStore::new(root.to_owned());
    let intents = root.join(INTENTS);
    let mut plan = HashMap::new();
    if regular_directory(&intents)? {
        for entry in fs::read_dir(&intents)? {
            let entry = entry?;
            let intent: DeleteIntent = serde_json::from_slice(&read_regular(&entry.path(), 4096)?)
                .map_err(io::Error::other)?;
            intent.validate()?;
            if entry.file_name() != format!("{}.json", intent.sha256).as_str() {
                return Err(io::Error::other("deletion intent filename mismatch"));
            }
            if retained.contains(&intent.sha256) {
                fs::remove_file(entry.path())?;
            } else {
                let target = store.path_for(&intent.sha256)?;
                if regular_directory(&root.join("objects"))?
                    && regular_directory(&root.join("objects/sha256"))?
                    && regular_directory(target.parent().unwrap())?
                {
                    match fs::symlink_metadata(&target) {
                        Ok(metadata) if metadata.is_file() => {
                            if metadata
                                .modified()?
                                .elapsed()
                                .map_or(true, |elapsed| elapsed < age)
                            {
                                continue; // same bytes may have been reimported after a crash
                            }
                        }
                        Ok(_) => {
                            return Err(io::Error::other("replayed deletion target is not regular"))
                        }
                        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                        Err(error) => return Err(error),
                    }
                }
                plan.insert(intent.sha256.clone(), intent);
            }
        }
    }
    let objects = root.join("objects/sha256");
    if !regular_directory(&root.join("objects"))? || !regular_directory(&objects)? {
        return Ok(plan.into_values().collect());
    }
    for shard in fs::read_dir(objects)? {
        let shard = shard?;
        if !shard.file_type()?.is_dir() {
            continue;
        }
        let Some(prefix) = shard.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if prefix.len() != 2 || !files::hash_valid(&prefix, 2) {
            continue;
        }
        for entry in fs::read_dir(shard.path())? {
            let entry = entry?;
            let Some(hash) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            if !files::hash_valid(&hash, 64)
                || !hash.starts_with(&prefix)
                || hash != hash.to_ascii_lowercase()
                || retained.contains(&hash)
                || plan.contains_key(&hash)
                || !entry.file_type()?.is_file()
            {
                continue;
            }
            let metadata = entry.metadata()?;
            if metadata
                .modified()?
                .elapsed()
                .map_or(true, |elapsed| elapsed < age)
            {
                continue;
            }
            let (digests, _) =
                files::digest::digest_file(&store.path_for(&hash)?, &Cancellation::new(), |_| {})?;
            if digests.sha256 != hash {
                continue;
            } // preserve unknown/corrupt bytes
            let intent = DeleteIntent {
                version: 1,
                sha256: hash.clone(),
                sha1: digests.sha1,
                size: digests.size_bytes,
            };
            fs::create_dir_all(&intents)?;
            files::atomic_write(
                &intents.join(format!("{hash}.json")),
                &serde_json::to_vec(&intent).map_err(io::Error::other)?,
            )
            .map_err(io::Error::other)?;
            #[cfg(test)]
            async_std::task::block_on(tests::checkpoint(root, "intent"));
            plan.insert(hash, intent);
        }
    }
    Ok(plan.into_values().collect())
}

fn unlink(maintenance: &StoreMaintenance, intent: &DeleteIntent) -> io::Result<bool> {
    intent.validate()?;
    let root = maintenance.root();
    let path = ObjectStore::new(root.to_owned()).path_for(&intent.sha256)?;
    let directories_present = regular_directory(&root.join("objects"))?
        && regular_directory(&root.join("objects/sha256"))?
        && regular_directory(path.parent().unwrap())?;
    let removed = if !directories_present {
        false
    } else {
        match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.is_file() => {
                let (digests, _) = files::digest::digest_file(&path, &Cancellation::new(), |_| {})?;
                if digests.sha256 != intent.sha256
                    || digests.sha1 != intent.sha1
                    || digests.size_bytes != intent.size
                {
                    return Err(io::Error::other("object changed since deletion intent"));
                }
                fs::remove_file(path)?;
                #[cfg(test)]
                async_std::task::block_on(tests::checkpoint(root, "unlink"));
                true
            }
            Ok(_) => return Err(io::Error::other("deletion target is not regular")),
            Err(error) if error.kind() == io::ErrorKind::NotFound => false,
            Err(error) => return Err(error),
        }
    };
    if regular_directory(&root.join("aliases"))? && regular_directory(&root.join("aliases/sha1"))? {
        let alias = root.join("aliases/sha1").join(&intent.sha1);
        match read_regular(&alias, 64) {
            Ok(body) if body == intent.sha256.as_bytes() => fs::remove_file(alias)?,
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    #[cfg(test)]
    async_std::task::block_on(tests::checkpoint(root, "alias"));
    Ok(removed)
}

fn clear_finished_intents(
    maintenance: &StoreMaintenance,
    completed: &[DeleteIntent],
) -> io::Result<()> {
    let intents = maintenance.root().join(INTENTS);
    if !regular_directory(&intents)? {
        return Ok(());
    }
    for intent in completed {
        intent.validate()?;
        if !ObjectStore::new(maintenance.root().to_owned())
            .path_for(&intent.sha256)?
            .try_exists()?
        {
            fs::remove_file(intents.join(format!("{}.json", intent.sha256)))?;
        }
    }
    Ok(())
}
