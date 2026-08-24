use std::{
    collections::{HashSet, VecDeque},
    path::PathBuf,
    sync::Arc,
    time::Duration,
};

use async_std::task::{self, JoinHandle};
use async_trait::async_trait;
use futures::{stream::FuturesUnordered, StreamExt};
use thiserror::Error;

use crate::{
    data::{
        db::DbError,
        registry::operation::{
            handle::OperationHandle,
            message::{
                stage::OperationStage,
                target::{FileStatus, ProcessTarget},
            },
        },
    },
    utils::download::{buffer::BufferPool, error::DownloadError, Download, Downloadable},
};

pub const DEFAULT_CONCURRENT_TASKS_COUNT: usize = 100;
pub const DEFAULT_BUFFERS_SIZE: usize = 16 * 1024; // 16 KiB each

pub const MAX_ATTEMPTS: usize = 3;

pub const RETRY_BACKOFF: Duration = Duration::from_millis(500);

#[derive(Debug, Error)]
pub enum SyncError {
    #[error(transparent)]
    Store(#[from] DbError),

    #[error("{failed} of {total} downloads failed; first error: {first}")]
    DownloadsFailed {
        failed: usize,
        total: usize,
        #[source]
        first: DownloadError,
    },
}

#[async_trait]
pub trait SyncStore<T>: Send + Sync {
    async fn cached(&self, wanted: &[T]) -> Result<HashSet<String>, DbError>;
    async fn register(&self, items: &[T]) -> Result<(), DbError>;
}

#[derive(Debug)]
pub struct SyncOutcome<T> {
    pub cached: Vec<T>,
    pub downloaded: Vec<T>,
}

impl<T> SyncOutcome<T> {
    pub fn all(&self) -> impl Iterator<Item = &T> {
        self.cached.iter().chain(self.downloaded.iter())
    }
}

pub struct Syncer {
    concurrency: usize,
    buffers: Arc<BufferPool>,
}

impl Default for Syncer {
    fn default() -> Self {
        Self::new(DEFAULT_CONCURRENT_TASKS_COUNT, DEFAULT_BUFFERS_SIZE)
    }
}

impl Syncer {
    pub fn new(concurrency: usize, buffer_size: usize) -> Self {
        Self {
            concurrency,
            buffers: Arc::new(BufferPool::new(concurrency, buffer_size)),
        }
    }

    pub async fn run<T, F>(
        &self,
        wanted: Vec<T>,
        save_path: F,
        store: &dyn SyncStore<T>,
        op: &OperationHandle,
        stage: OperationStage,
    ) -> Result<SyncOutcome<T>, SyncError>
    where
        T: Downloadable + Clone + Send + Sync + 'static,
        F: Fn(&T) -> PathBuf,
    {
        let wanted = dedup_by_hash(wanted);
        let total = wanted.len();

        let known = store.cached(&wanted).await?;
        let (cached, missing): (Vec<T>, Vec<T>) = wanted
            .into_iter()
            .partition(|item| known.contains(item.get_hash()));

        let cached_count = cached.len();

        op.update_stage(stage.clone(), cached_count, total, None);

        let mut downloaded: Vec<T> = Vec::with_capacity(missing.len());
        let mut failures: Vec<DownloadError> = Vec::new();

        let mut queue: VecDeque<(T, usize)> = missing.into_iter().map(|item| (item, 0)).collect();
        let mut futures = FuturesUnordered::new();

        loop {
            while futures.len() < self.concurrency {
                let Some((item, attempt)) = queue.pop_front() else {
                    break;
                };

                futures.push(self.spawn_attempt(save_path(&item), item, attempt));
            }

            let Some((item, attempt, error)) = futures.next().await else {
                break;
            };

            let Some(e) = error else {
                let name = item.get_name().clone();
                downloaded.push(item);

                op.update_stage(
                    stage.clone(),
                    cached_count + downloaded.len(),
                    total,
                    Some(ProcessTarget::file(name, FileStatus::Downloaded)),
                );

                continue;
            };

            let done = attempt + 1;

            if e.is_transient() && done < MAX_ATTEMPTS {
                tracing::warn!(
                    name = %item.get_name(),
                    attempt = done,
                    error = %e,
                    "download failed, retrying"
                );

                queue.push_back((item, done));
                continue;
            }

            tracing::warn!(name = %item.get_name(), attempts = done, error = %e, "download failed");
            failures.push(e);
        }

        if !downloaded.is_empty() {
            store.register(&downloaded).await?;
        }

        let failed = failures.len();

        if let Some(first) = failures.into_iter().next() {
            return Err(SyncError::DownloadsFailed {
                failed,
                total,
                first,
            });
        }

        Ok(SyncOutcome { cached, downloaded })
    }

    fn spawn_attempt<T>(
        &self,
        save_path: PathBuf,
        item: T,
        attempt: usize,
    ) -> JoinHandle<(T, usize, Option<DownloadError>)>
    where
        T: Downloadable + Clone + Send + Sync + 'static,
    {
        let fallback = item.clone();
        let download = Download::new(save_path, item, Arc::clone(&self.buffers));

        task::spawn(async move {
            if attempt > 0 {
                task::sleep(RETRY_BACKOFF * 2u32.pow(attempt as u32 - 1)).await;
            }

            match download.download_with_checksum().await {
                Ok(item) => (item, attempt, None),
                Err(e) => (fallback, attempt, Some(e)),
            }
        })
    }
}

#[cfg(test)]
mod tests;

fn dedup_by_hash<T: Downloadable>(items: Vec<T>) -> Vec<T> {
    let mut seen: HashSet<String> = HashSet::new();

    items
        .into_iter()
        .filter(|item| seen.insert(item.get_hash().clone()))
        .collect()
}
