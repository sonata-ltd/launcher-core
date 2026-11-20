use thiserror::Error;

#[derive(Debug, Error)]
pub enum ContentError {
    #[error("content resource is busy: {0}")]
    Busy(#[from] BusyReason),
    #[error("content plan is stale: {0:?}")]
    StalePlan(StalePlanReason),
    #[error(transparent)]
    Incompatible(#[from] super::resolve::graph::ResolveError),
    #[error("installed release {0:?} has no dependency metadata")]
    MissingDependencyMetadata(super::model::ReleaseKey),
    #[error(transparent)]
    Cancelled(#[from] crate::utils::cancellation::Cancelled),
    #[error("instance {0} requires content recovery")]
    RecoveryPending(crate::instance::model::InstanceId),
    #[error("content planning timed out after {0:?}")]
    Timeout(std::time::Duration),
    #[error("content worker panicked")]
    WorkerPanicked,
    #[error("content job {0} was not found")]
    JobNotFound(uuid::Uuid),
    #[error(transparent)]
    Task(#[from] crate::utils::jobs::supervisor::TaskError),
    #[error(transparent)]
    Instance(Box<crate::instance::error::InstanceError>),
    #[error("{0}")]
    Invalid(String),
    #[error("{0}")]
    CandidateUnavailable(String),
    #[error("content catalog request limit ({0}) reached")]
    CatalogRequestLimit(usize),
    #[error("unsupported content journal version {0}")]
    UnsupportedJournalVersion(u32),
    #[error("RecoveryConflict: {name} differs from the unfinished publication")]
    RecoveryConflict { name: String },
    #[error("staged bytes for {name} differ from the registered content digest")]
    StagedDigestMismatch { name: String },
    #[error("content path has an ambiguous binding: {path}", path = .path.display())]
    AmbiguousBinding { path: std::path::PathBuf },
    #[error("invalid content journal operation id {0:?}")]
    InvalidJournalId(String),
    #[error("CurseForge API credential is not configured")]
    MissingCurseForgeKey,
    #[error("content catalog returned HTTP {0}")]
    Http(u16),
    #[error("{0}")]
    Transport(#[source] crate::utils::download::json::JsonRequestError),
    #[error(transparent)]
    File(#[from] crate::utils::files::FileError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Database(#[from] sqlx::Error),
    #[error(transparent)]
    Download(#[from] crate::utils::download::transfer::error::DownloadError),
}

#[derive(Debug, thiserror::Error)]
pub enum BusyReason {
    #[error("active job limit {0}")]
    ActiveLimit(usize),
    #[error("preview limit {0}")]
    PreviewLimit(usize),
    #[error(transparent)]
    Instance(Box<crate::instance::error::InstanceError>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StalePlanReason {
    Expired,
    InventoryChanged,
    RegisteredContentChanged,
    CompatibilityChanged,
}

impl From<crate::instance::error::InstanceError> for ContentError {
    fn from(error: crate::instance::error::InstanceError) -> Self {
        match error {
            error @ (crate::instance::error::InstanceError::Busy(_)
            | crate::instance::error::InstanceError::AlreadyRunning(_)) => {
                Self::Busy(BusyReason::Instance(Box::new(error)))
            }
            other => Self::Instance(Box::new(other)),
        }
    }
}

impl ContentError {
    pub fn is_cancelled(&self) -> bool {
        matches!(self, Self::Cancelled(_))
            || matches!(self, Self::Download(error) if error.is_cancelled())
    }
}

/// Only construction failures belong to bootstrap, not catalog/planning errors.
#[derive(Debug, thiserror::Error)]
pub enum ContentInitError {
    #[error("content lock {} is unavailable: {source}", path.display())]
    Lock {
        path: std::path::PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    File(#[from] crate::utils::files::FileError),
    #[error(transparent)]
    Database(#[from] sqlx::Error),
    #[error(transparent)]
    Task(#[from] crate::utils::jobs::supervisor::TaskError),
    #[error("content ownership initialization: {0}")]
    Ownership(#[source] Box<dyn std::error::Error + Send + Sync>),
}

impl From<ContentError> for ContentInitError {
    fn from(error: ContentError) -> Self {
        match error {
            ContentError::Io(error) => Self::Io(error),
            ContentError::File(error) => Self::File(error),
            ContentError::Database(error) => Self::Database(error),
            other => Self::Ownership(Box::new(other)),
        }
    }
}

pub type Result<T> = std::result::Result<T, ContentError>;

#[cfg(test)]
mod tests {
    use super::*;
    use std::error::Error;

    #[test]
    fn cancellation_is_typed_and_busy_preserves_instance_identity() {
        assert!(ContentError::from(crate::utils::cancellation::Cancelled).is_cancelled());
        assert!(!ContentError::Invalid("operation cancelled".into()).is_cancelled());
        let error = ContentError::from(crate::instance::error::InstanceError::Busy(42.into()));
        assert!(
            matches!(error, ContentError::Busy(BusyReason::Instance(ref source)) if matches!(**source, crate::instance::error::InstanceError::Busy(id) if id.get() == 42))
        );
        assert!(error.source().is_some());
    }

    #[test]
    fn content_transport_keeps_decode_source_and_bootstrap_has_only_construction_categories() {
        let json = serde_json::from_str::<serde_json::Value>("not JSON").unwrap_err();
        let error =
            ContentError::from(crate::utils::download::json::JsonRequestError::Decode(json));
        assert!(matches!(
            error,
            ContentError::Transport(crate::utils::download::json::JsonRequestError::Decode(_))
        ));
        assert!(error.source().is_some());
        let init = ContentInitError::from(ContentError::Database(sqlx::Error::PoolClosed));
        assert!(matches!(
            init,
            ContentInitError::Database(sqlx::Error::PoolClosed)
        ));
        assert!(matches!(
            ContentError::StalePlan(StalePlanReason::Expired),
            ContentError::StalePlan(_)
        ));
    }
}
