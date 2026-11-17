//! Watches recently used, registered mod inventories and reconciles them on disk changes.
use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

use crate::{
    data::db::Database,
    instance::{activity::InstanceActivity, model::InstanceId},
    utils::files,
};

use super::{
    error::{ContentError, Result},
    inventory,
    scan_policy::DirtyGeneration,
    storage,
    watch::ContentWatcher,
};

const MAX_WATCHES: usize = 16;
const DEBOUNCE: Duration = Duration::from_millis(150);

#[derive(Default)]
pub(in crate::instance) struct ModIndex {
    watches: Mutex<VecDeque<WatchedInstance>>,
}

struct WatchedInstance {
    instance: InstanceId,
    root: PathBuf,
    dirty: Arc<DirtyGeneration>,
    _watcher: Arc<Mutex<ContentWatcher>>,
}

impl ModIndex {
    /// Called with the launch reservation held. Notifications are advisory,
    /// so every launch with registered mods verifies the current bytes.
    pub(in crate::instance) async fn before_launch(
        &self,
        db: &Arc<Database>,
        activity: &Arc<InstanceActivity>,
        instance: InstanceId,
        root: PathBuf,
    ) -> Result<()> {
        if !storage::has_registered_mods(db, instance).await? {
            return Ok(());
        }
        let dirty = self.watch(db, activity, instance, &root);
        reconcile(db, instance, &root, dirty.as_deref()).await
    }

    pub(in crate::instance) fn forget(&self, instance: InstanceId) {
        if let Ok(mut watches) = self.watches.lock() {
            watches.retain(|entry| entry.instance != instance);
        }
    }

    pub(super) fn watch(
        &self,
        db: &Arc<Database>,
        activity: &Arc<InstanceActivity>,
        instance: InstanceId,
        root: &Path,
    ) -> Option<Arc<DirtyGeneration>> {
        let mut watches = self.watches.lock().ok()?;
        if let Some(index) = watches.iter().position(|entry| entry.instance == instance) {
            let entry = watches.remove(index)?;
            if entry.root == root {
                let dirty = Arc::clone(&entry.dirty);
                watches.push_back(entry);
                return Some(dirty);
            }
        }

        match start_watch(db.clone(), activity.clone(), instance, root.to_owned()) {
            Ok(entry) => {
                let dirty = Arc::clone(&entry.dirty);
                if watches.len() == MAX_WATCHES {
                    watches.pop_front();
                }
                watches.push_back(entry);
                Some(dirty)
            }
            Err(error) => {
                tracing::debug!(%instance, %error, "mod watch unavailable");
                None
            }
        }
    }
}

fn start_watch(
    db: Arc<Database>,
    activity: Arc<InstanceActivity>,
    instance: InstanceId,
    root: PathBuf,
) -> Result<WatchedInstance> {
    let (wake, changes) = async_channel::bounded(1);
    let dirty = Arc::new(DirtyGeneration::default());
    let mods = files::child(&root, Path::new("mods"))?;
    let events_root = root.clone();
    let events_mods = mods.clone();
    let mut watcher = ContentWatcher::with_wake(
        dirty.clone(),
        move |event| {
            event.paths.is_empty()
                || event
                    .paths
                    .iter()
                    .any(|path| path == &events_root || path.starts_with(&events_mods))
        },
        move || {
            let _ = wake.try_send(());
        },
    )
    .map_err(std::io::Error::other)?;
    watcher
        .watch_directory(&root)
        .map_err(std::io::Error::other)?;
    let mut mods_identity = directory_identity(&mods);
    if mods_identity.is_some() {
        watcher
            .watch_directory(&mods)
            .map_err(std::io::Error::other)?;
    }
    let watcher = Arc::new(Mutex::new(watcher));
    let weak_watcher = Arc::downgrade(&watcher);

    let watched_root = root.clone();
    let watched_dirty = dirty.clone();
    async_std::task::spawn(async move {
        while changes.recv().await.is_ok() {
            async_std::task::sleep(DEBOUNCE).await;
            while changes.try_recv().is_ok() {}
            if changes.is_closed() {
                break;
            }
            let current_identity = directory_identity(&mods);
            if current_identity != mods_identity {
                let Some(watcher) = weak_watcher.upgrade() else {
                    break;
                };
                if let Ok(mut watcher) = watcher.lock() {
                    if mods_identity.is_some() {
                        watcher.forget_directory(&mods);
                    }
                    mods_identity =
                        current_identity.filter(|_| watcher.watch_directory(&mods).is_ok());
                };
            }
            // Running a game is allowed to defer the index update. Launch and
            // installation both perform a fresh scan under their reservation.
            let Ok(_activity) = activity.reserve_content_change(instance) else {
                continue;
            };
            if let Err(error) = reconcile(&db, instance, &watched_root, Some(&watched_dirty)).await
            {
                tracing::debug!(%instance, %error, "mod index refresh deferred");
            }
        }
    });

    Ok(WatchedInstance {
        instance,
        root,
        dirty,
        _watcher: watcher,
    })
}

fn directory_identity(path: &Path) -> Option<files::digest::FileStamp> {
    let metadata = std::fs::symlink_metadata(path).ok()?;
    metadata
        .is_dir()
        .then(|| files::digest::FileStamp::from_metadata(&metadata))
}

async fn reconcile(
    db: &Database,
    instance: InstanceId,
    root: &Path,
    dirty: Option<&DirtyGeneration>,
) -> Result<()> {
    for _ in 0..2 {
        let generation = dirty.map(DirtyGeneration::current);
        let snapshot = match std::fs::symlink_metadata(root) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                inventory::ModSnapshot::default()
            }
            _ => inventory::snapshot(root.to_owned()).await?,
        };
        if let Some((dirty, generation)) = dirty.zip(generation) {
            if dirty.changed_since(generation) {
                continue;
            }
        }
        return storage::reconcile_registered_mods(db, instance, &snapshot).await;
    }
    Err(ContentError::Invalid(
        "mods changed during inventory verification".into(),
    ))
}
