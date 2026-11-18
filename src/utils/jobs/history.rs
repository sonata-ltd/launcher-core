//! Completed records have their own ring/TTL. They never consume active-job
//! admission, and expiration starts at completion rather than at job creation.
use chrono::{DateTime, Utc};
use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

#[derive(Clone)]
pub(crate) struct Finished<K, V> {
    pub id: K,
    pub value: V,
    pub finished_at: DateTime<Utc>,
    completed: Instant,
}

pub(crate) struct History<K, V> {
    capacity: usize,
    ttl: Duration,
    records: VecDeque<Finished<K, V>>,
}

impl<K: PartialEq, V> History<K, V> {
    pub(crate) fn new(capacity: usize, ttl: Duration) -> Self {
        Self {
            capacity,
            ttl,
            records: VecDeque::new(),
        }
    }

    pub(crate) fn push(&mut self, id: K, value: V) {
        self.push_at(id, value, Instant::now(), Utc::now());
    }

    fn push_at(&mut self, id: K, value: V, now: Instant, finished_at: DateTime<Utc>) {
        self.prune_at(now);
        self.records.retain(|record| record.id != id);
        if self.capacity == 0 {
            return;
        }
        while self.records.len() >= self.capacity {
            self.records.pop_front();
        }
        self.records.push_back(Finished {
            id,
            value,
            finished_at,
            completed: now,
        });
    }

    pub(crate) fn prune(&mut self) {
        self.prune_at(Instant::now());
    }

    fn prune_at(&mut self, now: Instant) {
        self.records
            .retain(|record| now.saturating_duration_since(record.completed) < self.ttl);
    }

    pub(crate) fn records(&self) -> impl Iterator<Item = &Finished<K, V>> {
        self.records.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capacity_evicts_oldest_completion_without_rejecting_new_records() {
        let mut history = History::new(2, Duration::from_secs(3600));
        for id in 0..1000 {
            history.push(id, id);
        }
        assert_eq!(
            history
                .records()
                .map(|record| record.id)
                .collect::<Vec<_>>(),
            [998, 999]
        );
        history.push(998, 7);
        assert_eq!(
            history
                .records()
                .map(|record| (record.id, record.value))
                .collect::<Vec<_>>(),
            [(999, 999), (998, 7)]
        );
    }

    #[test]
    fn long_running_job_gets_full_ttl_after_completion_and_zero_capacity_is_safe() {
        let mut history = History::new(2, Duration::from_secs(60));
        let created = Instant::now();
        let completed = created + Duration::from_secs(7200);
        let stamp = Utc::now();
        history.push_at(1, "finished", completed, stamp);
        history.prune_at(completed + Duration::from_secs(59));
        assert_eq!(history.records().next().unwrap().finished_at, stamp);
        history.prune_at(completed + Duration::from_secs(60));
        assert!(history.records().next().is_none());
        let mut empty = History::new(0, Duration::ZERO);
        empty.push(1, "no retention");
        assert!(empty.records().next().is_none());
    }
}
