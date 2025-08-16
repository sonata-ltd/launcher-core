use std::ops::{Deref, DerefMut};

use async_channel::{bounded, Receiver, Sender, TrySendError};
use async_std::task;

/// Pool of reusable buffers
/// - capacity: how many buffers in the pool
/// - buf_size: size of each buffer in bytes
#[derive(Clone)]
pub struct BufferPool {
    tx: Sender<Vec<u8>>,
    rx: Receiver<Vec<u8>>
}

impl BufferPool {
    /// Create a pool with `capacity` buffers of size `buf_size`
    pub fn new(capacity: usize, buf_size: usize) -> Self {
        let (tx, rx) = bounded(capacity);

        for _ in 0..capacity {
            let buf = vec![0u8; buf_size];
            tx.try_send(buf).expect("Initial fill shound succeed");
        }

        BufferPool { tx, rx }
    }

    pub async fn acquire(&self) -> BufferGuard {
        // Wait until buffer is available
        let buf = self.rx.recv().await.expect("Pool recieve closed");
        BufferGuard {
            buf: Some(buf),
            tx: self.tx.clone()
        }
    }
}


/// Guard that holds a buffer and returns it to the pool on Drop
pub struct BufferGuard {
    buf: Option<Vec<u8>>,
    tx: Sender<Vec<u8>>
}

#[allow(dead_code)]
impl BufferGuard {
    /// Get mutable slice to pass into async reads
    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        self.buf.as_mut().expect("Buffer consumed").as_mut_slice()
    }

    /// Take ownership of the inner Vec (consumes the guard without returning it to pool).
    /// Useful if you want to keep the buffer (then you should send it back manually)
    pub fn into_inner(mut self) -> Vec<u8> {
        self.buf.take().unwrap()
    }

    /// Return the buffer to the pool explicitly (async).
    pub async fn release(mut self) {
        if let Some(buf) = self.buf.take() {
            // Send back, awaiting if necessary
            let _ = self.tx.send(buf).await;
        }
    }
}

impl Drop for BufferGuard {
    fn drop(&mut self) {
        if let Some(buf) = self.buf.take() {
            // Non-blocking return to pool
            match self.tx.try_send(buf) {
                Ok(()) => {},
                Err(TrySendError::Full(buf)) => {
                    // Extremely unlikely: pool is full. Spawn background task to send it asynchronously
                    let tx_clone = self.tx.clone();
                    task::spawn(async move {
                        // Ignore error if channel closed
                        let _ = tx_clone.send(buf).await;
                    });
                }
                Err(TrySendError::Closed(_buf)) => {
                    // Channel closed, drop buffer
                }
            }
        }
    }
}

impl Deref for BufferGuard {
    type Target = Vec<u8>;
    fn deref(&self) -> &Self::Target {
        self.buf.as_ref().expect("Buffer consumed")
    }
}

impl DerefMut for BufferGuard {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.buf.as_mut().expect("Buffer consumed")
    }
}
