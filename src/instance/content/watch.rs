use std::{
    path::Path,
    sync::{mpsc::SyncSender, Arc},
};

use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};

use crate::instance::content::scan_policy::DirtyGeneration;

pub struct ContentWatcher {
    watcher: RecommendedWatcher,
}

impl ContentWatcher {
    pub fn new(dirty: Arc<DirtyGeneration>, wake: SyncSender<()>) -> notify::Result<Self> {
        let watcher =
            notify::recommended_watcher(
                move |result: notify::Result<notify::Event>| match result {
                    Ok(event) => {
                        if event.need_rescan() || !matches!(event.kind, EventKind::Access(_)) {
                            dirty.mark_changed();
                            let _ = wake.try_send(());
                        }
                    }
                    Err(_) => {
                        dirty.mark_changed();
                        let _ = wake.try_send(());
                    }
                },
            )?;

        Ok(Self { watcher })
    }

    pub fn watch_directory(&mut self, directory: &Path) -> notify::Result<()> {
        self.watcher.watch(directory, RecursiveMode::NonRecursive)
    }
}
