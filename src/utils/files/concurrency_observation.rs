//! Repeated observation of launcher-owned layout under concurrent directory
//! replacement. Ignored by default; run explicitly with `--ignored`.
//! The control scenario uses path resolution and must observe escapes, which
//! shows the harness exercises the window; held-directory scenarios must not.

use std::{
    fs,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread,
};

use super::directory::Directory;
use crate::utils::{cancellation::Cancellation, object_store::ObjectStore};

pub(crate) const REPEATS: usize = 7;
pub(crate) const ATTEMPTS: usize = 400;

/// Keeps replacing `anchor/owned` with a link to `outside` until dropped.
pub(crate) struct Replacer {
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
    owned: PathBuf,
    real: PathBuf,
}

impl Replacer {
    pub(crate) fn start(owned: PathBuf, outside: PathBuf) -> Self {
        let real = owned.with_extension("real");
        let stop = Arc::new(AtomicBool::new(false));
        let worker = {
            let (stop, owned, real) = (stop.clone(), owned.clone(), real.clone());
            thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    if fs::rename(&owned, &real).is_ok() {
                        let _ = std::os::unix::fs::symlink(&outside, &owned);
                        thread::yield_now();
                        let _ = fs::remove_file(&owned);
                        let _ = fs::rename(&real, &owned);
                    }
                    thread::yield_now();
                }
            })
        };
        Self {
            stop,
            worker: Some(worker),
            owned,
            real,
        }
    }
}

impl Drop for Replacer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        if fs::symlink_metadata(&self.owned).is_ok_and(|m| m.file_type().is_symlink()) {
            let _ = fs::remove_file(&self.owned);
        }
        if self.real.exists() {
            let _ = fs::rename(&self.real, &self.owned);
        }
    }
}

pub(crate) fn files_below(root: &Path) -> usize {
    let mut count = 0;
    if let Ok(entries) = fs::read_dir(root) {
        for entry in entries.flatten() {
            let kind = entry.file_type().unwrap();
            if kind.is_dir() {
                count += files_below(&entry.path());
            } else {
                count += 1;
            }
        }
    }
    count
}

/// Runs `attempt` repeatedly under replacement; returns (outside, inside)
/// file counts per repeat. `layout` exists both inside `owned` and in the decoy.
fn observe(layout: &str, attempt: impl Fn(&Path, usize)) -> Vec<(usize, usize)> {
    (0..REPEATS)
        .map(|_| {
            let temp = tempfile::tempdir().unwrap();
            let anchor = temp.path().join("anchor");
            let outside = temp.path().join("outside");
            fs::create_dir_all(anchor.join("owned").join(layout)).unwrap();
            fs::create_dir_all(outside.join(layout)).unwrap();
            {
                let _replacer = Replacer::start(anchor.join("owned"), outside.clone());
                for index in 0..ATTEMPTS {
                    attempt(&anchor, index);
                }
            }
            (files_below(&outside), files_below(&anchor.join("owned")))
        })
        .collect()
}

/// Returns repeats with files outside the root. Every repeat must also have
/// completed work inside the root, or a zero would be meaningless.
fn report(name: &str, counts: &[(usize, usize)], require_inside: bool) -> usize {
    let repeats = counts.iter().filter(|(outside, _)| *outside > 0).count();
    eprintln!(
        "{name}: repeats with files outside the root {repeats}/{REPEATS}, (outside, inside) {counts:?}"
    );
    if require_inside {
        assert!(
            counts.iter().all(|(_, inside)| *inside > 0),
            "{name}: no completed work"
        );
    }
    repeats
}

#[test]
#[ignore = "concurrency observation; run explicitly"]
fn control_path_resolution_is_observed_outside_the_root() {
    let escaped = observe("inner", |anchor, index| {
        let path = anchor.join("owned/inner");
        let _ = fs::create_dir_all(&path)
            .and_then(|()| Directory::open(&path))
            .and_then(|directory| directory.write_bytes(format!("f{index}").as_ref(), b"bytes"));
    });
    assert!(
        report("control", &escaped, false) > 0,
        "harness did not reach the window"
    );
}

#[test]
#[ignore = "concurrency observation; run explicitly"]
fn held_directories_stay_inside_the_root_under_replacement() {
    let escaped = observe("inner", |anchor, index| {
        let _ =
            Directory::open_beneath(anchor, Path::new("owned/inner"), true).and_then(|directory| {
                directory
                    .ok_or_else(|| std::io::ErrorKind::NotFound.into())
                    .and_then(|directory| {
                        directory.write_bytes(format!("f{index}").as_ref(), b"bytes")
                    })
            });
    });
    assert_eq!(report("open_beneath", &escaped, true), 0);
}

#[test]
#[ignore = "concurrency observation; run explicitly"]
fn object_store_stays_inside_the_root_under_replacement() {
    let sources = tempfile::tempdir().unwrap();
    let escaped = observe("store", |anchor, index| {
        let source = sources.path().join(format!("s{index}"));
        fs::write(&source, format!("bytes {index}")).unwrap();
        if let Ok(store) = ObjectStore::beneath(anchor, Path::new("owned/store")) {
            let _ = store.import_file(&source, None, &Cancellation::new());
        }
    });
    assert_eq!(report("object_store", &escaped, true), 0);
}

#[test]
#[ignore = "concurrency observation; run explicitly"]
fn process_pins_stay_inside_the_root_under_replacement() {
    let escaped = observe("inner", |anchor, _| {
        if let Ok(Some(directory)) = Directory::open_beneath(anchor, Path::new("owned/inner"), true)
        {
            if let Ok(pin) = crate::utils::process_pin::ProcessPin::create_in(&directory) {
                pin.finish(true);
            }
        }
    });
    assert_eq!(report("process_pin", &escaped, true), 0);
}
