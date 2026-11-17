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
        Self::with_wake(
            dirty,
            |_| true,
            move || {
                let _ = wake.try_send(());
            },
        )
    }

    pub(super) fn with_wake(
        dirty: Arc<DirtyGeneration>,
        relevant: impl Fn(&notify::Event) -> bool + Send + 'static,
        wake: impl Fn() + Send + 'static,
    ) -> notify::Result<Self> {
        let watcher = notify::recommended_watcher(move |result: notify::Result<notify::Event>| {
            let changed = result.map_or(true, |event| {
                event.need_rescan()
                    || (!matches!(event.kind, EventKind::Access(_)) && relevant(&event))
            });
            if changed {
                dirty.mark_changed();
                wake();
            }
        })?;

        Ok(Self { watcher })
    }

    pub(super) fn forget_directory(&mut self, directory: &Path) {
        let _ = self.watcher.unwatch(directory);
    }

    pub fn watch_directory(&mut self, directory: &Path) -> notify::Result<()> {
        self.watcher.watch(directory, RecursiveMode::NonRecursive)
    }
}
