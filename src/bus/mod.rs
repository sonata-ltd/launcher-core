use async_broadcast::{InactiveReceiver, Receiver, Sender, TrySendError};

use crate::bus::event::CoreEvent;

pub mod event;

#[derive(Clone)]
pub struct EventBus {
    tx: Sender<CoreEvent>,
    _keepalive: InactiveReceiver<CoreEvent>,
}

impl EventBus {
    pub fn new(cap: usize) -> Self {
        let (mut tx, rx) = async_broadcast::broadcast(cap);

        tx.set_overflow(true);
        tx.set_await_active(false);

        Self {
            tx,
            _keepalive: rx.deactivate(),
        }
    }

    pub fn publish(&self, event: impl Into<CoreEvent>) {
        match self.tx.try_broadcast(event.into()) {
            Ok(None) => {}

            Ok(Some(_evicted)) => {
                tracing::trace!("event bus overflow, oldest event dropped");
            }

            Err(TrySendError::Inactive(_)) => {}

            Err(TrySendError::Full(_)) => {
                tracing::warn!("event bus is full, event dropped");
            }

            Err(TrySendError::Closed(_)) => {
                tracing::error!("event bus is closed, event dropped");
            }
        }
    }

    pub fn subscribe(&self) -> Receiver<CoreEvent> {
        self.tx.new_receiver()
    }
}

#[cfg(test)]
mod tests {
    use crate::instance::scan::{ScanData, ScanIntegrity, ScanMessage};

    use super::*;

    #[async_std::test]
    async fn bus_survives_having_no_subs() {
        let bus = EventBus::new(4);

        bus.publish(ScanMessage {
            data: ScanData {
                integrity: ScanIntegrity {
                    instance_path: None,
                },
                info: None,
            },
        });

        let mut rx = bus.subscribe();
        bus.publish(ScanMessage {
            data: ScanData {
                integrity: ScanIntegrity {
                    instance_path: Some("some_path".to_string()),
                },
                info: None,
            },
        });

        assert!(rx.recv().await.is_ok());
    }
}
