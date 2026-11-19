//! Asset CAS retention. Minecraft never executes/reads these canonical objects:
//! it reads isolated SHA-1 placements, which this collector never deletes.
//! Conservative roots include every asset DB row, index checksum and placement.
use super::storage::bytes_error;
use crate::{
    data::db::{retention::LockedDatabases, Database, DbError},
    utils::{
        cancellation::Cancellation,
        files,
        object_store::{
            catalog::{read_regular, RootCatalog},
            collection::{Collection, Roots},
            ObjectStore,
        },
    },
};
use std::{
    fs, io,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

type Result<T> = std::result::Result<T, DbError>;

mod index;

pub(super) async fn attach(
    db: Arc<Database>,
    objects: PathBuf,
    cancel: Cancellation,
) -> Result<()> {
    async_std::task::spawn(async move {
        let root = files::child(&objects, Path::new(".sonata-cas")).map_err(io::Error::other)?;
        let store = ObjectStore::new(root);
        let access = store.access(&cancel).await?;
        let owner = db.retention_owner().await.ok().flatten();
        if RootCatalog::covers(access.root(), owner.as_ref())? {
            return Ok(());
        }
        drop(access);
        let maintenance = store.maintenance(&cancel).await?;
        let owner = match db.retention_owner().await {
            Ok(owner) => owner,
            Err(error) => {
                tracing::warn!(%error, "unknown asset database owner disables collection");
                None
            }
        };
        RootCatalog::attach(&maintenance, owner)?;
        Ok(())
    })
    .await
    .map_err(bytes_error)
}

pub(super) async fn collect(assets: PathBuf, minimum_age: Duration) -> Result<usize> {
    async_std::task::spawn(async move {
        let assets = fs::canonicalize(assets).map_err(bytes_error)?;
        let root = files::child(&assets, Path::new("objects/.sonata-cas"))
            .map_err(|error| bytes_error(io::Error::other(error)))?;
        let maintenance = Arc::new(
            ObjectStore::new(root)
                .try_maintenance()
                .map_err(bytes_error)?,
        );
        let mut databases = LockedDatabases::open(maintenance.clone()).await?;
        let mut roots = Roots::default();
        for tx in &mut databases.transactions {
            let hashes: Vec<String> = sqlx::query_scalar("SELECT hash FROM assets")
                .fetch_all(&mut **tx)
                .await?;
            for hash in hashes {
                add_hash(&mut roots, &hash).map_err(bytes_error)?;
            }
        }
        #[cfg(test)]
        tests::snapshot_checkpoint(maintenance.root()).await;
        let collection = Collection::new(maintenance);
        #[cfg(test)]
        let collection = collection.with_test_hook(tests::checkpoint);
        #[cfg(test)]
        let checkpoint_root = assets.join("objects/.sonata-cas");
        let result = async_std::task::spawn_blocking(move || {
            placement_roots(&assets, &mut roots)?;
            let plan = collection.plan(&roots, minimum_age)?;
            let mut removed = 0;
            for intent in &plan {
                removed += usize::from(collection.unlink(intent)?);
            }
            collection.clear_finished_intents(&plan)?;
            #[cfg(test)]
            tests::checkpoint(&checkpoint_root, "cleaned");
            Ok(removed)
        })
        .await
        .map_err(bytes_error);
        databases.close().await;
        result
    })
    .await
}

fn add_hash(roots: &mut Roots, hash: &str) -> io::Result<()> {
    if !files::hash_valid(hash, 40) {
        return Err(io::Error::other("invalid asset root SHA-1"));
    }
    roots.sha1.insert(hash.to_ascii_lowercase());
    Ok(())
}

fn directory(path: &Path) -> io::Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() => Ok(true),
        Ok(_) => Err(io::Error::other("asset retention directory is not regular")),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

fn placement_roots(assets: &Path, roots: &mut Roots) -> io::Result<()> {
    if !directory(assets)? {
        return Err(io::Error::other("missing asset root"));
    }
    for entry in fs::read_dir(assets)? {
        let entry = entry?;
        match entry.file_name().to_str() {
            Some("objects" | "indexes") if entry.file_type()?.is_dir() => {}
            // Virtual/resource projections are kept, but their unknown ownership
            // must not be used to declare even the private CAS unrooted.
            _ => return Err(io::Error::other("unknown asset layout blocks collection")),
        }
    }
    let indexes = assets.join("indexes");
    if directory(&indexes)? {
        for entry in fs::read_dir(indexes)? {
            let entry = entry?;
            let index: index::Index =
                serde_json::from_slice(&read_regular(&entry.path(), 32 * 1024 * 1024)?)
                    .map_err(io::Error::other)?;
            for object in index.objects {
                add_hash(roots, &object.hash)?;
            }
        }
    }
    let objects = assets.join("objects");
    if directory(&objects)? {
        for shard in fs::read_dir(objects)? {
            let shard = shard?;
            if shard.file_name() == ".sonata-cas" {
                if !shard.file_type()?.is_dir() {
                    return Err(io::Error::other("asset CAS root is not regular"));
                }
                for entry in fs::read_dir(shard.path())? {
                    let entry = entry?;
                    let kind = entry.file_type()?;
                    match entry.file_name().to_str() {
                        Some(
                            ".sonata-access.lock" | ".sonata-store.lock" | ".sonata-roots.json",
                        ) if kind.is_file() => {}
                        Some("objects" | "aliases" | ".sonata-gc") if kind.is_dir() => {}
                        Some("staging")
                            if kind.is_dir() && fs::read_dir(entry.path())?.next().is_none() => {}
                        _ => return Err(io::Error::other("unknown or pending asset CAS layout")),
                    }
                }
                continue;
            }
            let prefix = shard
                .file_name()
                .into_string()
                .map_err(|_| io::Error::other("non-Unicode asset shard"))?;
            if !files::hash_valid(&prefix, 2)
                || prefix != prefix.to_ascii_lowercase()
                || !shard.file_type()?.is_dir()
            {
                return Err(io::Error::other("unknown asset shard blocks collection"));
            }
            for entry in fs::read_dir(shard.path())? {
                let entry = entry?;
                let hash = entry
                    .file_name()
                    .into_string()
                    .map_err(|_| io::Error::other("non-Unicode asset placement"))?;
                if !files::hash_valid(&hash, 40)
                    || !hash.starts_with(&prefix)
                    || hash != hash.to_ascii_lowercase()
                    || !entry.file_type()?.is_file()
                {
                    return Err(io::Error::other(
                        "unknown asset placement blocks collection",
                    ));
                }
                // Even a corrupt placement is an intended semantic checksum and
                // may need a valid CAS for repair. Never infer garbage from bytes.
                add_hash(roots, &hash)?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
