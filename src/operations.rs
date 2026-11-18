//! Read-only operation contracts. Mutation/admission belongs to domain services.
//!
//! Initial subscribe and overflow recovery use `GlobalState::operations()`.
//! Snapshots/history do not pin bytes or grant permission to collect files.
pub use crate::data::registry::operation::subject::OperationSubject;
pub use crate::data::registry::operation::{
    message, snapshot::OperationSnapshot, JobId, OperationReconciliation, OperationResult,
};
pub use crate::utils::jobs::supervisor::{TaskError, TaskResult};
