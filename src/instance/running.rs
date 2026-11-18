use std::{
    collections::HashMap,
    io,
    process::ExitStatus,
    sync::{Arc, Mutex},
};

use async_std::{
    channel::{self, Receiver, Sender},
    process::Child,
    task,
};
use futures::future::{self, Either};

use crate::{
    bus::EventBus,
    instance::{
        error::{InstanceError, Result},
        event::InstanceEvent,
        launch::natives::NativeDirectory,
        model::InstanceId,
    },
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunPhase {
    Preparing,
    Running { pid: u32 },
}

struct Entry {
    phase: RunPhase,
    /// `None` while preparing: there is no process to kill yet.
    kill: Option<Sender<()>>,
}

#[derive(Clone)]
pub(crate) struct RunningInstances {
    tasks: crate::utils::jobs::supervisor::Tasks,
    inner: Arc<Mutex<HashMap<InstanceId, Entry>>>,
    bus: EventBus,
}

impl RunningInstances {
    pub fn new(bus: EventBus) -> Self {
        Self::with_tasks(bus, Default::default())
    }

    pub(crate) fn with_tasks(bus: EventBus, tasks: crate::utils::jobs::supervisor::Tasks) -> Self {
        Self {
            inner: Arc::new(Mutex::new(HashMap::new())),
            bus,
            tasks,
        }
    }

    pub fn reserve(&self, id: InstanceId) -> Result<Reservation> {
        let ticket = self
            .tasks
            .admit("game preparation/process", None)
            .map_err(InstanceError::Task)?;
        {
            let mut map = self.inner.lock().unwrap();

            if map.contains_key(&id) {
                return Err(InstanceError::AlreadyRunning(id));
            }

            map.insert(
                id,
                Entry {
                    phase: RunPhase::Preparing,
                    kill: None,
                },
            );
        }

        self.bus.publish(InstanceEvent::PreparingStart(id));

        Ok(Reservation {
            id,
            owner: Some(self.clone()),
            ticket: Some(ticket),
        })
    }

    pub fn snapshot(&self) -> Vec<(InstanceId, RunPhase)> {
        self.inner
            .lock()
            .unwrap()
            .iter()
            .map(|(id, entry)| (*id, entry.phase))
            .collect()
    }

    pub fn phase(&self, id: InstanceId) -> Option<RunPhase> {
        self.inner.lock().unwrap().get(&id).map(|e| e.phase)
    }

    pub fn kill(&self, id: InstanceId) -> Result<()> {
        let tx = self
            .inner
            .lock()
            .unwrap()
            .get(&id)
            .and_then(|e| e.kill.clone())
            .ok_or(InstanceError::NotRunning(id))?;

        let _ = tx.try_send(());

        Ok(())
    }

    fn remove(&self, id: &InstanceId) {
        self.inner.lock().unwrap().remove(&id);
    }
}

pub(crate) struct Reservation {
    id: InstanceId,
    owner: Option<RunningInstances>,
    ticket: Option<crate::utils::jobs::supervisor::Ticket>,
}

impl Reservation {
    pub fn commit(self, child: Child) {
        self.commit_process(child, None);
    }

    pub(crate) fn commit_with_natives(self, child: Child, natives: NativeDirectory) {
        self.commit_process(child, Some(natives));
    }

    fn commit_process(mut self, child: Child, natives: Option<NativeDirectory>) {
        let owner = self.owner.take().expect("a reservation commits once");
        let id = self.id;
        let pid = child.id();
        let (tx, rx) = channel::bounded(1);

        if let Some(entry) = owner.inner.lock().unwrap().get_mut(&id) {
            entry.phase = RunPhase::Running { pid };
            entry.kill = Some(tx);
        }

        owner.bus.publish(InstanceEvent::Started { id, pid });

        // A running game is not force-stopped by launcher shutdown. Its watcher
        // must still confirm exit/cleanup before shutdown reports completion.
        self.ticket
            .take()
            .expect("admitted game preparation")
            .spawn(watch(owner, id, child, rx, natives));
    }
}

impl Drop for Reservation {
    fn drop(&mut self) {
        if let Some(owner) = self.owner.take() {
            owner.remove(&self.id);
            owner.bus.publish(InstanceEvent::LaunchFailed(self.id));
        }
    }
}

enum Wake {
    Exited(io::Result<ExitStatus>),
    Kill,
}

async fn watch(
    owner: RunningInstances,
    id: InstanceId,
    mut child: Child,
    kill: Receiver<()>,
    natives: Option<NativeDirectory>,
) {
    let wake = {
        let exited = child.status();
        let killed = async {
            match kill.recv().await {
                Ok(()) => (),
                Err(_) => future::pending().await,
            }
        };

        futures::pin_mut!(exited, killed);

        match future::select(exited, killed).await {
            Either::Left((status, _)) => Wake::Exited(status),
            Either::Right(_) => Wake::Kill,
        }
    };

    let status = match wake {
        Wake::Exited(status) => status,
        Wake::Kill => {
            if let Err(e) = child.kill() {
                tracing::warn!(%id, "failed to kill game process: {e}");
            }

            child.status().await
        }
    };

    if let Some(natives) = natives {
        let exited = status.is_ok();
        task::spawn_blocking(move || natives.finish(exited)).await;
    }

    let code = match status {
        Ok(status) => {
            tracing::info!(%id, %status, "game was stopped");
            status.code()
        }
        Err(e) => {
            tracing::error!(%id, "failed to wait for game process: {e}");
            None
        }
    };

    owner.remove(&id);
    owner.bus.publish(InstanceEvent::Exited { id, code });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bus::event::CoreEvent;
    use crate::utils::process::tests::waiting_child;

    #[async_std::test]
    async fn shutdown_waits_for_admitted_preparation_and_confirmed_process_exit() {
        let tasks = crate::utils::jobs::supervisor::Tasks::default();
        let running = RunningInstances::with_tasks(EventBus::new(8), tasks.clone());
        let reservation = running.reserve(1.into()).unwrap();
        let mut shutdown = Box::pin(tasks.shutdown());
        assert!(futures::poll!(&mut shutdown).is_pending());
        assert!(running.reserve(2.into()).is_err());
        let (_child_root, child) = waiting_child().await;
        reservation.commit(child);
        assert!(futures::poll!(&mut shutdown).is_pending());
        running.kill(1.into()).unwrap();
        async_std::future::timeout(std::time::Duration::from_secs(3), shutdown)
            .await
            .unwrap();
        assert!(running.snapshot().is_empty());
    }

    async fn next_instance_event(rx: &mut async_broadcast::Receiver<CoreEvent>) -> InstanceEvent {
        loop {
            if let CoreEvent::Instance(event) = rx.recv().await.unwrap() {
                return event;
            }
        }
    }

    #[async_std::test]
    async fn a_second_launch_is_refused_until_the_first_reservation_drops() {
        let running = RunningInstances::new(EventBus::new(16));
        let id = InstanceId::from(1);

        let first = running.reserve(id).unwrap();
        assert!(matches!(
            running.reserve(id),
            Err(InstanceError::AlreadyRunning(_))
        ));

        drop(first);
        assert!(running.phase(id).is_none(), "a dropped claim is released");
        assert!(running.reserve(id).is_ok());
    }

    #[async_std::test]
    async fn a_dropped_reservation_reports_a_failed_launch() {
        let bus = EventBus::new(16);
        let mut rx = bus.subscribe();
        let running = RunningInstances::new(bus);
        let id = InstanceId::from(1);

        drop(running.reserve(id).unwrap());

        assert!(matches!(
            next_instance_event(&mut rx).await,
            InstanceEvent::PreparingStart(_)
        ));
        assert!(matches!(
            next_instance_event(&mut rx).await,
            InstanceEvent::LaunchFailed(_)
        ));
    }

    #[async_std::test]
    async fn the_watcher_reports_the_exit_code_and_clears_the_entry() {
        let bus = EventBus::new(16);
        let mut rx = bus.subscribe();
        let running = RunningInstances::new(bus);
        let id = InstanceId::from(1);

        let (child_root, child) = waiting_child().await;
        running.reserve(id).unwrap().commit(child);
        std::fs::write(child_root.path().join("exit"), b"3").unwrap();

        let _ = next_instance_event(&mut rx).await; // Preparing
        assert!(matches!(
            next_instance_event(&mut rx).await,
            InstanceEvent::Started { .. }
        ));
        let exited = async_std::future::timeout(
            std::time::Duration::from_secs(10),
            next_instance_event(&mut rx),
        )
        .await
        .unwrap();
        assert!(matches!(
            exited,
            InstanceEvent::Exited { code: Some(3), .. }
        ));
        assert!(running.snapshot().is_empty());
    }

    #[async_std::test]
    async fn native_directory_survives_running_process_and_is_removed_before_exit_event() {
        let root = tempfile::tempdir().unwrap();
        let mut natives = NativeDirectory::create(root.path()).unwrap();
        let path = natives.as_ref().to_owned();
        std::fs::write(path.join("libtest.so"), b"test").unwrap();
        let bus = EventBus::new(16);
        let mut rx = bus.subscribe();
        let running = RunningInstances::new(bus);
        let id = 1.into();
        let (_child_root, child) = waiting_child().await;
        let runtime_pin = crate::utils::process_pin::ProcessPin::create(root.path()).unwrap();
        runtime_pin.before_spawn().unwrap();
        runtime_pin.record_child(child.id());
        natives.retain_runtime(runtime_pin);
        running
            .reserve(id)
            .unwrap()
            .commit_with_natives(child, natives);
        assert!(path.join("libtest.so").exists());
        assert!(!crate::utils::process_pin::allows_cleanup(root.path()).unwrap());
        running.kill(id).unwrap();
        async_std::future::timeout(std::time::Duration::from_secs(3), async {
            while !matches!(
                next_instance_event(&mut rx).await,
                InstanceEvent::Exited { .. }
            ) {}
        })
        .await
        .unwrap();
        assert!(!path.exists());
        assert!(running.phase(id).is_none());
        assert!(crate::utils::process_pin::allows_cleanup(root.path()).unwrap());
    }

    #[async_std::test]
    async fn kill_ends_the_process() {
        let bus = EventBus::new(16);
        let mut rx = bus.subscribe();
        let running = RunningInstances::new(bus);
        let id = InstanceId::from(1);

        let (_child_root, child) = waiting_child().await;
        running.reserve(id).unwrap().commit(child);
        running.kill(id).unwrap();

        async_std::future::timeout(std::time::Duration::from_secs(10), async {
            loop {
                if let InstanceEvent::Exited { code, .. } = next_instance_event(&mut rx).await {
                    #[cfg(unix)]
                    assert_eq!(code, None, "killed by a signal, so no exit code");
                    #[cfg(windows)]
                    assert!(code.is_some(), "Windows termination reports an exit code");
                    break;
                }
            }
        })
        .await
        .unwrap();
        assert!(matches!(
            running.kill(id),
            Err(InstanceError::NotRunning(_))
        ));
    }
}
