use futures::future::{select, Either};
use std::{future::Future, io, pin::pin};

#[derive(Clone)]
pub struct Cancellation {
    sender: async_channel::Sender<()>,
    receiver: async_channel::Receiver<()>,
}

#[derive(Debug, Clone, Copy, thiserror::Error)]
#[error("operation cancelled")]
pub struct Cancelled;

impl From<Cancelled> for io::Error {
    fn from(error: Cancelled) -> Self {
        Self::new(io::ErrorKind::Interrupted, error)
    }
}

impl Default for Cancellation {
    fn default() -> Self {
        Self::new()
    }
}

impl Cancellation {
    pub fn new() -> Self {
        let (sender, receiver) = async_channel::bounded(1);
        Self { sender, receiver }
    }

    pub fn cancel(&self) {
        // Closing permanently wakes all current and future waiters.
        self.sender.close();
    }

    pub fn is_cancelled(&self) -> bool {
        self.receiver.is_closed()
    }

    pub fn check(&self) -> Result<(), Cancelled> {
        if self.is_cancelled() {
            return Err(Cancelled);
        }
        Ok(())
    }

    pub async fn cancelled(&self) {
        let _ = self.receiver.recv().await;
    }

    pub async fn run<T>(&self, operation: impl Future<Output = T>) -> Result<T, Cancelled> {
        match select(pin!(self.cancelled()), pin!(operation)).await {
            Either::Left(_) => Err(Cancelled),
            Either::Right((value, _)) => Ok(value),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{future, time::Duration};

    #[async_std::test]
    async fn cancellation_interrupts_all_pending_operations_and_later_calls() {
        let token = Cancellation::new();
        let second = token.clone();
        let trigger = token.clone();
        let results = async_std::future::timeout(Duration::from_secs(1), async {
            futures::join!(
                token.run(future::pending::<()>()),
                second.run(future::pending::<()>()),
                async {
                    async_std::task::yield_now().await;
                    trigger.cancel();
                }
            )
        })
        .await
        .unwrap();
        assert!(matches!(results.0, Err(Cancelled)));
        assert!(matches!(results.1, Err(Cancelled)));
        assert!(matches!(token.run(future::ready(())).await, Err(Cancelled)));
    }
}
