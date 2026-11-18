//! Cooperative shutdown of owned work. Cancellation is a signal, never dropping
//! a publishing future. Tickets survive caller drop and cover cleanup/SQL commits.
use super::history::History;
use crate::utils::cancellation::Cancellation;
use futures::FutureExt;
use std::{
    collections::HashMap,
    future::Future,
    sync::{Arc, Mutex},
    time::Duration,
};
use uuid::Uuid;

#[derive(Debug, Clone, thiserror::Error)]
pub enum TaskError {
    #[error("launcher is shutting down")]
    ShuttingDown,
    #[error("background task panicked: {0}")]
    Panicked(String),
}

#[derive(Debug, Clone)]
pub struct TaskResult {
    pub name: String,
    pub error: Option<TaskError>,
}

struct Active {
    name: String,
    cancel: Option<Cancellation>,
}
struct State {
    closing: bool,
    active: HashMap<Uuid, Active>,
    history: History<Uuid, TaskResult>,
    changed: Cancellation,
}

#[derive(Clone)]
pub struct Tasks {
    state: Arc<Mutex<State>>,
}

impl Default for Tasks {
    fn default() -> Self {
        Self {
            state: Arc::new(Mutex::new(State {
                closing: false,
                active: HashMap::new(),
                history: History::new(128, Duration::from_secs(3600)),
                changed: Cancellation::new(),
            })),
        }
    }
}

impl Tasks {
    pub fn spawn<F, T>(
        &self,
        name: &str,
        cancel: Option<Cancellation>,
        future: F,
    ) -> Result<async_std::task::JoinHandle<Result<T, TaskError>>, TaskError>
    where
        F: Future<Output = T> + Send + 'static,
        T: Send + 'static,
    {
        Ok(self.admit(name, cancel)?.spawn(future))
    }

    pub(crate) fn admit(
        &self,
        name: &str,
        cancel: Option<Cancellation>,
    ) -> Result<Ticket, TaskError> {
        let id = Uuid::new_v4();
        {
            let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
            if state.closing {
                return Err(TaskError::ShuttingDown);
            }
            state.active.insert(
                id,
                Active {
                    name: name.into(),
                    cancel,
                },
            );
        }
        Ok(Ticket {
            tasks: self.clone(),
            id,
            error: None,
        })
    }

    /// Idempotently seal admission, signal cancellable work, and await *all*
    /// admitted work, including non-cancellable publication/cleanup. Dropping a
    /// shutdown caller does not reopen admission or drop any worker.
    pub async fn shutdown(&self) -> Vec<TaskResult> {
        loop {
            let changed = {
                let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
                state.closing = true;
                for task in state.active.values() {
                    if let Some(cancel) = &task.cancel {
                        cancel.cancel();
                    }
                }
                if state.active.is_empty() {
                    state.history.prune();
                    return state
                        .history
                        .records()
                        .map(|record| record.value.clone())
                        .collect();
                }
                state.changed.clone()
            };
            changed.cancelled().await;
        }
    }
}

pub(crate) struct Ticket {
    tasks: Tasks,
    id: Uuid,
    error: Option<TaskError>,
}
impl Ticket {
    pub(crate) fn spawn<F, T>(self, future: F) -> async_std::task::JoinHandle<Result<T, TaskError>>
    where
        F: Future<Output = T> + Send + 'static,
        T: Send + 'static,
    {
        async_std::task::spawn(async move {
            let mut ticket = self;
            match std::panic::AssertUnwindSafe(future).catch_unwind().await {
                Ok(value) => Ok(value),
                Err(_) => {
                    let error = TaskError::Panicked("see panic diagnostics".into());
                    ticket.error = Some(error.clone());
                    Err(error)
                }
            }
        })
    }
}
impl Drop for Ticket {
    fn drop(&mut self) {
        let mut state = self
            .tasks
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(active) = state.active.remove(&self.id) {
            state.history.push(
                self.id,
                TaskResult {
                    name: active.name,
                    error: self.error.take(),
                },
            );
        }
        let changed = std::mem::replace(&mut state.changed, Cancellation::new());
        changed.cancel();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[async_std::test]
    async fn shutdown_signals_download_but_waits_for_owned_commit_after_caller_drop() {
        let tasks = Tasks::default();
        let cancel = Cancellation::new();
        let token = cancel.clone();
        let (entered, started) = async_channel::bounded(1);
        let (resume, release) = async_channel::bounded(1);
        let handle = tasks
            .spawn("download then commit", Some(cancel), async move {
                token.cancelled().await;
                entered.send(()).await.unwrap();
                release.recv().await.unwrap();
            })
            .unwrap();
        drop(handle);
        let mut shutdown = Box::pin(tasks.shutdown());
        assert!(futures::poll!(&mut shutdown).is_pending());
        started.recv().await.unwrap();
        assert!(futures::poll!(&mut shutdown).is_pending());
        assert!(matches!(
            tasks.spawn("late", None, async {}),
            Err(TaskError::ShuttingDown)
        ));
        resume.send(()).await.unwrap();
        assert_eq!(shutdown.await.len(), 1);
        assert_eq!(tasks.shutdown().await.len(), 1);
    }

    #[async_std::test]
    async fn panic_is_observable_and_concurrent_shutdown_waiters_both_finish() {
        let tasks = Tasks::default();
        let handle = tasks
            .spawn("panic fixture", None, async {
                panic!("fixture");
            })
            .unwrap();
        assert!(matches!(handle.await, Err(TaskError::Panicked(_))));
        let (a, b) = futures::join!(tasks.shutdown(), tasks.shutdown());
        assert_eq!(a.len(), 1);
        assert_eq!(b.len(), 1);
        assert!(matches!(a[0].error, Some(TaskError::Panicked(_))));
    }
}
