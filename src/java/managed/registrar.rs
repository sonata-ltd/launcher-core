//! Managed validation owns probing and its durable Ready record. Registration
//! belongs to the surrounding Java domain, supplied as an owned narrow port.
use super::Result;
use crate::java::{model::JavaRuntime, utils::DetectedJavaRuntime};
use async_trait::async_trait;
use std::sync::Arc;

#[async_trait]
pub trait RuntimeRegistrar: Send + Sync {
    /// Resolve a durable runtime identity, not an unretained executable handle.
    async fn runtime(&self, id: i64) -> Result<Option<Arc<JavaRuntime>>>;

    /// Return the existing runtime for this executable or register it as managed.
    /// Must tolerate retry after SQL success followed by Ready-record failure.
    async fn register_managed(&self, detected: DetectedJavaRuntime) -> Result<Arc<JavaRuntime>>;

    /// Called only after the Ready record is durable and visible to queries.
    fn validation_committed(&self, runtime: &JavaRuntime);
}
