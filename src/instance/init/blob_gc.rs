//! Runtime blob retention: current profile/legacy checksums plus every byte in
//! surviving owned generations. Live/unknown preparations disable this domain,
//! even when they have not yet materialized their next blob as a Maven file.
use super::{error::Result, generation_gc, retention};
use crate::{
    data::layout::LauncherPaths,
    utils::object_store::{
        catalog::RootCatalog,
        collection::{Collection, Roots},
        ObjectStore,
    },
};
use std::{collections::HashSet, fs, io, sync::Arc, time::Duration};

pub(super) async fn collect(paths: Arc<LauncherPaths>, minimum_age: Duration) -> Result<usize> {
    async_std::task::spawn(async move {
        let snapshot = retention::snapshot(&paths).await?;
        let library = snapshot.maintenance.clone();
        let blobs = library.root().join("blobs");
        match fs::symlink_metadata(&blobs) {
            Ok(metadata) if metadata.is_dir() => {}
            Ok(_) => return Err(io::Error::other("runtime blob root is not regular").into()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                snapshot.databases.close().await;
                return Ok(0);
            }
            Err(error) => return Err(error.into()),
        }
        // Lock order is library -> blobs, as in admitted installation. Standalone
        // blob importers are excluded too; try rather than hold SQL while waiting.
        let maintenance = Arc::new(ObjectStore::child_of(&blobs)?.try_maintenance()?);
        let owners: HashSet<_> = RootCatalog::owners(&library)?
            .into_iter()
            .map(|owner| owner.identity)
            .collect();
        let mut hashes = snapshot.sha1;
        let roots = async_std::task::spawn_blocking(move || {
            generation_gc::blob_roots(library.root(), &owners, &mut hashes)?;
            Ok::<_, io::Error>(hashes)
        })
        .await
        .map(|sha1| Roots {
            sha1,
            ..Roots::default()
        })?;
        #[cfg(test)]
        tests::checkpoint(maintenance.root(), "snapshot").await;
        let collection = Collection::new(maintenance.clone())?;
        #[cfg(test)]
        let collection = collection.with_test_hook(|root, phase| {
            async_std::task::block_on(tests::checkpoint(root, phase))
        });
        let worker = collection.clone();
        let plan =
            async_std::task::spawn_blocking(move || worker.plan(&roots, minimum_age)).await?;
        let mut removed = 0;
        for intent in &plan {
            let worker = collection.clone();
            let intent = intent.clone();
            removed +=
                usize::from(async_std::task::spawn_blocking(move || worker.unlink(&intent)).await?);
        }
        let worker = collection.clone();
        async_std::task::spawn_blocking(move || worker.clear_finished_intents(&plan)).await?;
        #[cfg(test)]
        tests::checkpoint(maintenance.root(), "cleaned").await;
        snapshot.databases.close().await;
        Ok(removed)
    })
    .await
}

#[cfg(test)]
mod tests;
