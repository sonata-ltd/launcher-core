//! Runtime ownership catalogue and current-profile roots gate. Generation/blob
//! deletion is deliberately separate and must consume this protected snapshot.
use super::{
    error::{InitError, Result},
    model::InstalledProfile,
};
use crate::{
    data::{
        db::{retention::LockedDatabases, Database, DbError},
        layout::LauncherPaths,
    },
    utils::{
        cancellation::Cancellation,
        files,
        object_store::{catalog::RootCatalog, ObjectStore, StoreMaintenance},
        process_pin,
    },
};
use sqlx::{Sqlite, Transaction};
use std::{
    collections::HashSet,
    path::{Component, Path, PathBuf},
    sync::Arc,
};

pub(crate) async fn attach(
    db: Arc<Database>,
    libraries: PathBuf,
    cancel: Cancellation,
) -> Result<()> {
    // SQL and maintenance outlive a dropped caller while identity is resolved.
    async_std::task::spawn(async move {
        let store = ObjectStore::new(libraries);
        // Already catalogued writers must not wait for another installation's
        // whole shared workspace lifetime just to rewrite the same catalogue.
        let access = store.access(&cancel).await?;
        let owner = db.retention_owner().await.ok().flatten();
        if RootCatalog::covers(access.root(), owner.as_ref())? {
            return Ok(());
        }
        drop(access);
        let maintenance = store.maintenance(&cancel).await?;
        #[cfg(test)]
        tests::checkpoint(maintenance.root(), "attach").await;
        let owner = match db.retention_owner().await {
            Ok(owner) => owner,
            Err(error) => {
                RootCatalog::attach(&maintenance, None)?;
                tracing::warn!(%error, "unknown runtime database owner disables collection");
                return Ok(());
            }
        };
        RootCatalog::attach(&maintenance, owner)?;
        Ok(())
    })
    .await
}

pub(crate) async fn initialize(db: Arc<Database>, paths: Arc<LauncherPaths>) -> Result<()> {
    attach(db.clone(), paths.libraries().clone(), Cancellation::new()).await?;
    match super::library_paths::migrate(db, paths.libraries().clone()).await {
        Ok(report) => tracing::debug!(
            converted = report.converted,
            unresolved = report.unresolved,
            "legacy library paths inspected"
        ),
        Err(error) => tracing::debug!(%error, "legacy library path upgrade deferred"),
    }
    // Unknown historical ownership and live/unknown pins conservatively defer.
    match super::generation_gc::collect(paths.clone(), std::time::Duration::from_secs(7 * 86400))
        .await
    {
        Ok(removed) => tracing::debug!(removed, "runtime generation maintenance completed"),
        Err(error) => {
            tracing::debug!(%error, "runtime retention gate deferred");
            return Ok(());
        }
    }
    match super::blob_gc::collect(paths, std::time::Duration::from_secs(7 * 86400)).await {
        Ok(removed) => tracing::debug!(removed, "runtime blob maintenance completed"),
        Err(error) => tracing::debug!(%error, "runtime blob retention gate deferred"),
    }
    Ok(())
}

pub(crate) struct RuntimeSnapshot {
    pub(crate) generations: HashSet<PathBuf>,
    pub(crate) sha1: HashSet<String>,
    pub(crate) databases: LockedDatabases,
    pub(crate) maintenance: Arc<StoreMaintenance>,
}

pub(crate) async fn snapshot(paths: &LauncherPaths) -> Result<RuntimeSnapshot> {
    let store = ObjectStore::new(paths.libraries().clone());
    let maintenance = Arc::new(store.try_maintenance()?);
    if !process_pin::allows_cleanup(maintenance.root())? {
        return Err(InitError::Io(std::io::Error::other(
            "live or unknown game pin blocks runtime collection",
        )));
    }
    let mut databases = LockedDatabases::open(maintenance.clone())
        .await
        .map_err(DbError::from)?;
    let mut generations = HashSet::new();
    let mut sha1 = HashSet::new();
    for tx in &mut databases.transactions {
        let (current, hashes) = roots(tx, maintenance.root()).await?;
        generations.extend(current);
        sha1.extend(hashes);
    }
    Ok(RuntimeSnapshot {
        generations,
        sha1,
        databases,
        maintenance,
    })
}

async fn roots(
    tx: &mut Transaction<'_, Sqlite>,
    root: &Path,
) -> Result<(HashSet<PathBuf>, HashSet<String>)> {
    let mut roots = HashSet::new();
    let mut hashes = HashSet::new();
    let profiles: Vec<(i64, String)> =
        sqlx::query_as("SELECT instance_id,profile_json FROM instance_installations")
            .fetch_all(&mut **tx)
            .await
            .map_err(DbError::from)?;
    for (instance, body) in profiles {
        let json: serde_json::Value =
            serde_json::from_str(&body).map_err(|_| DbError::ResultCorrupted)?;
        if json.as_object().is_none_or(|object| {
            object.keys().any(|key| {
                ![
                    "format_version",
                    "minecraft",
                    "loader",
                    "manifest",
                    "java",
                    "libraries_root",
                    "artifacts",
                    "metadata",
                ]
                .contains(&key.as_str())
            })
        }) {
            return Err(DbError::ResultCorrupted.into());
        }
        if json
            .get("artifacts")
            .and_then(|value| value.as_array())
            .is_none_or(|artifacts| {
                artifacts.iter().any(|artifact| {
                    artifact.as_object().is_none_or(|object| {
                        object.keys().any(|key| {
                            !["path", "sha1", "classpath", "native"].contains(&key.as_str())
                        })
                    })
                })
            })
        {
            return Err(DbError::ResultCorrupted.into());
        }
        let profile: InstalledProfile =
            serde_json::from_str(&body).map_err(|_| DbError::ResultCorrupted)?;
        if profile.format_version != 1 {
            return Err(DbError::InvalidInput(
                "unsupported installed profile retention schema".into(),
            )
            .into());
        }
        files::relative(&profile.libraries_root)?;
        files::child(root, &profile.libraries_root)?;
        let expected = PathBuf::from("instances").join(instance.to_string());
        if !profile.libraries_root.starts_with(&expected)
            || profile.libraries_root.components().count() != 4
            || profile.libraries_root.file_name() != Some("maven".as_ref())
        {
            return Err(DbError::ResultCorrupted.into());
        }
        // Validate even disabled/non-classpath artifacts. Unknown paths/hashes
        // cannot be interpreted as absence of a runtime root.
        for artifact in profile.artifacts {
            files::relative(&artifact.path)?;
            if !files::hash_valid(&artifact.sha1, 40) {
                return Err(DbError::ResultCorrupted.into());
            }
            hashes.insert(artifact.sha1.to_ascii_lowercase());
            files::child(root, &profile.libraries_root.join(&artifact.path))?;
        }
        if let Some(generation) = generation(&profile.libraries_root)? {
            roots.insert(generation);
        } else {
            return Err(DbError::InvalidInput(
                "installed profile has unknown generation layout".into(),
            )
            .into());
        }
    }
    let placements: Vec<(String, String)> = sqlx::query_as("SELECT path,hash FROM instance_library_placements UNION SELECT l.path,l.hash FROM libraries l JOIN instances_libraries i ON i.library_id=l.id")
        .fetch_all(&mut **tx).await.map_err(DbError::from)?;
    for (path, hash) in placements {
        if !files::hash_valid(&hash, 40) {
            return Err(DbError::ResultCorrupted.into());
        }
        hashes.insert(hash.to_ascii_lowercase());
        let path = Path::new(&path);
        let relative = if path.is_absolute() {
            path.strip_prefix(root).map_err(|_| {
                DbError::InvalidInput("legacy library path has unknown relocated ownership".into())
            })?
        } else {
            path
        };
        files::child(root, relative)?;
        if let Some(generation) = generation(relative)? {
            roots.insert(generation);
        }
    }
    Ok((roots, hashes))
}

fn generation(path: &Path) -> Result<Option<PathBuf>> {
    files::relative(path)?;
    let components: Vec<_> = path.components().collect();
    if components.first() != Some(&Component::Normal("instances".as_ref())) {
        return Ok(None);
    }
    if components.len() < 3 {
        return Err(DbError::ResultCorrupted.into());
    }
    let Component::Normal(instance) = components[1] else {
        unreachable!()
    };
    let Component::Normal(id) = components[2] else {
        unreachable!()
    };
    let instance = instance.to_str().ok_or(DbError::ResultCorrupted)?;
    let parsed: i64 = instance.parse().map_err(|_| DbError::ResultCorrupted)?;
    let id = id.to_str().ok_or(DbError::ResultCorrupted)?;
    let uuid = uuid::Uuid::parse_str(id).map_err(|_| DbError::ResultCorrupted)?;
    if parsed.to_string() != instance || uuid.to_string() != id {
        return Err(DbError::ResultCorrupted.into());
    }
    Ok(Some(PathBuf::from("instances").join(instance).join(id)))
}

#[cfg(test)]
mod tests;
