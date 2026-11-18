//! Operation-scoped test rendezvous; no timings or process-wide fault injection.
use futures::channel::oneshot;
use std::{
    collections::HashMap,
    sync::{Mutex, OnceLock},
    time::Duration,
};
use uuid::Uuid;

struct Hook {
    entered: oneshot::Sender<()>,
    resume: oneshot::Receiver<()>,
}

type Key = (Uuid, &'static str);
static HOOKS: OnceLock<Mutex<HashMap<Key, Hook>>> = OnceLock::new();

pub(in crate::instance::content) async fn wait(operation: Uuid, phase: &'static str) {
    let hook = HOOKS
        .get_or_init(Default::default)
        .lock()
        .unwrap()
        .remove(&(operation, phase));
    if let Some(hook) = hook {
        let _ = hook.entered.send(());
        let _ = hook.resume.await;
    }
}

pub(in crate::instance::content) struct Pause {
    key: Key,
    entered: oneshot::Receiver<()>,
    resume: Option<oneshot::Sender<()>>,
}

impl Pause {
    pub(in crate::instance::content) fn new(operation: Uuid, phase: &'static str) -> Self {
        let (entered, receiver) = oneshot::channel();
        let (resume, sender) = oneshot::channel();
        let key = (operation, phase);
        assert!(HOOKS
            .get_or_init(Default::default)
            .lock()
            .unwrap()
            .insert(
                key,
                Hook {
                    entered,
                    resume: sender
                }
            )
            .is_none());
        Self {
            key,
            entered: receiver,
            resume: Some(resume),
        }
    }

    pub(in crate::instance::content) async fn entered(&mut self) {
        async_std::future::timeout(Duration::from_secs(10), &mut self.entered)
            .await
            .expect("checkpoint timed out")
            .expect("worker did not reach checkpoint");
    }

    pub(in crate::instance::content) fn resume(mut self) {
        let _ = self.resume.take().unwrap().send(());
    }
}

impl Drop for Pause {
    fn drop(&mut self) {
        HOOKS
            .get_or_init(Default::default)
            .lock()
            .unwrap()
            .remove(&self.key);
    }
}
