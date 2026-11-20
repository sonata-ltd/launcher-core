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
        files,
        object_store::{
            catalog::{DatabaseOwner, RootCatalog},
            collection::{Collection, Roots},
            ObjectStore, StoreMaintenance,
        },
    },
};
#[cfg(test)]
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{Sqlite, Transaction};
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::Duration,
};

#[cfg(test)]
use crate::utils::{
    cancellation::Cancellation,
    object_store::collection::{DeleteIntent, INTENTS},
};
#[cfg(test)]
use std::{fs, path::Path};

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
    let collection = Collection::new(maintenance.clone())?;
    #[cfg(test)]
    let collection = collection
        .with_test_hook(|root, phase| async_std::task::block_on(tests::checkpoint(root, phase)));
    let worker = collection.clone();
    let plan = async_std::task::spawn_blocking(move || {
        worker.plan(
            &Roots {
                sha256: retained,
                ..Roots::default()
            },
            minimum_age,
        )
    })
    .await?;
    let mut removed = 0;
    for intent in &plan {
        let worker = collection.clone();
        let deletion = intent.clone();
        let was_removed = async_std::task::spawn_blocking(move || worker.unlink(&deletion)).await?;
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
    let worker = collection.clone();
    async_std::task::spawn_blocking(move || worker.clear_finished_intents(&plan)).await?;
    #[cfg(test)]
    tests::checkpoint(maintenance.root(), "cleaned").await;
    databases.close().await;
    Ok(removed)
}
