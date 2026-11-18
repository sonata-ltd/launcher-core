use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ArchiveFormat {
    Zip,
    TarGz,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JavaTarget {
    pub os: String,
    pub arch: String,
}

impl JavaTarget {
    pub fn host() -> Result<Self> {
        use crate::utils::platform::{Arch, HostPlatform, Os};

        let host = HostPlatform::current();
        let os = match host.os {
            Os::Linux if host.musl => "alpine-linux",
            Os::Linux => "linux",
            Os::Windows => "windows",
            Os::MacOs => "mac",
            Os::Other(other) => return Err(ManagedJavaError::new("unsupported_platform", other)),
        };
        let arch = match host.arch {
            Arch::X86_64 => "x64",
            Arch::X86 => "x32",
            Arch::Aarch64 => "aarch64",
            Arch::Arm => "arm",
            Arch::Other(other) => return Err(ManagedJavaError::new("unsupported_platform", other)),
        };
        Ok(Self {
            os: os.into(),
            arch: arch.into(),
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ManagedJavaPackage {
    pub id: String,
    pub version: String,
    pub major: u32,
    pub target: JavaTarget,
    pub image_type: String,
    pub url: String,
    pub size: u64,
    pub sha256: String,
    pub format: ArchiveFormat,
}

#[derive(Debug, Clone)]
pub struct ManagedJavaCatalog {
    pub packages: Vec<ManagedJavaPackage>,
    pub warnings: Vec<ManagedJavaCatalogWarning>,
}

#[derive(Debug, Clone)]
pub struct ManagedJavaCatalogWarning {
    pub major: u32,
    pub error: ManagedJavaError,
}

#[derive(Debug, thiserror::Error)]
#[error("all managed Java catalog requests failed: {warnings:?}")]
pub struct ManagedJavaCatalogFailures {
    pub warnings: Vec<ManagedJavaCatalogWarning>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ManagedJavaStage {
    Downloading,
    VerifyingChecksum,
    Extracting,
    AwaitingValidation,
    Validating,
    Ready,
    Cancelling,
    Cancelled,
    Failed,
}

impl ManagedJavaStage {
    pub fn is_active(self) -> bool {
        matches!(
            self,
            Self::Downloading
                | Self::VerifyingChecksum
                | Self::Extracting
                | Self::Validating
                | Self::Cancelling
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ManagedJavaSnapshot {
    pub id: Uuid,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub package: ManagedJavaPackage,
    pub stage: ManagedJavaStage,
    pub downloaded_bytes: u64,
    pub total_bytes: Option<u64>,
    pub runtime_id: Option<i64>,
    pub error: Option<ManagedJavaError>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ManagedRecoveryDiagnostic {
    pub path: std::path::PathBuf,
    pub operation: Option<Uuid>,
    pub error: ManagedJavaError,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManagedInstallationsReport {
    pub installations: Vec<ManagedJavaSnapshot>,
    pub diagnostics: Vec<ManagedRecoveryDiagnostic>,
}

/// Stable persisted codes. Unknown legacy/future codes round-trip unchanged.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "String", into = "String")]
pub enum ManagedJavaErrorKind {
    Cancelled,
    Catalog,
    Download,
    Checksum,
    Extract,
    Validation,
    Storage,
    Registration,
    Busy,
    NotFound,
    UnsupportedPlatform,
    RecoveryRecord,
    WorkerPanic,
    Shutdown,
    Unknown(String),
}

impl ManagedJavaErrorKind {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Cancelled => "cancelled",
            Self::Catalog => "catalog",
            Self::Download => "download",
            Self::Checksum => "checksum",
            Self::Extract => "extract",
            Self::Validation => "validation",
            Self::Storage => "storage",
            Self::Registration => "registration",
            Self::Busy => "busy",
            Self::NotFound => "not_found",
            Self::UnsupportedPlatform => "unsupported_platform",
            Self::RecoveryRecord => "recovery_record",
            Self::WorkerPanic => "worker_panic",
            Self::Shutdown => "shutdown",
            Self::Unknown(code) => code,
        }
    }
}
impl From<String> for ManagedJavaErrorKind {
    fn from(code: String) -> Self {
        Self::from(code.as_str())
    }
}
impl From<&str> for ManagedJavaErrorKind {
    fn from(code: &str) -> Self {
        match code {
            "cancelled" => Self::Cancelled,
            "catalog" => Self::Catalog,
            "download" => Self::Download,
            "checksum" => Self::Checksum,
            "extract" => Self::Extract,
            "validation" => Self::Validation,
            "storage" => Self::Storage,
            "registration" => Self::Registration,
            "busy" => Self::Busy,
            "not_found" => Self::NotFound,
            "unsupported_platform" => Self::UnsupportedPlatform,
            "recovery_record" => Self::RecoveryRecord,
            "worker_panic" => Self::WorkerPanic,
            "shutdown" => Self::Shutdown,
            other => Self::Unknown(other.into()),
        }
    }
}
impl From<ManagedJavaErrorKind> for String {
    fn from(code: ManagedJavaErrorKind) -> String {
        code.as_str().into()
    }
}
impl std::fmt::Display for ManagedJavaErrorKind {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}
// Compatibility for diagnostic consumers; domain policy uses enum variants.
impl PartialEq<&str> for ManagedJavaErrorKind {
    fn eq(&self, other: &&str) -> bool {
        self.as_str() == *other
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManagedJavaError {
    pub code: ManagedJavaErrorKind,
    pub stage: Option<ManagedJavaStage>,
    pub message: String,
    pub details: String,
    pub retryable: bool,
    /// Runtime sources are not reconstructed from persisted diagnostic text.
    #[serde(skip)]
    pub source: Option<std::sync::Arc<dyn std::error::Error + Send + Sync>>,
}

impl std::fmt::Display for ManagedJavaError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for ManagedJavaError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.source.as_deref().map(|source| source as _)
    }
}

impl PartialEq for ManagedJavaError {
    fn eq(&self, other: &Self) -> bool {
        self.code == other.code
            && self.stage == other.stage
            && self.message == other.message
            && self.details == other.details
            && self.retryable == other.retryable
    }
}
impl Eq for ManagedJavaError {}

impl From<crate::utils::cancellation::Cancelled> for ManagedJavaError {
    fn from(error: crate::utils::cancellation::Cancelled) -> Self {
        Self::caused_by(ManagedJavaErrorKind::Cancelled, error)
    }
}

impl ManagedJavaError {
    pub fn new(code: impl Into<ManagedJavaErrorKind>, details: impl std::fmt::Display) -> Self {
        let code = code.into();
        Self {
            stage: None,
            message: format!("managed Java {code} failure"),
            code,
            details: details.to_string(),
            retryable: false,
            source: None,
        }
    }

    pub fn caused_by<E>(code: impl Into<ManagedJavaErrorKind>, source: E) -> Self
    where
        E: std::error::Error + Send + Sync + 'static,
    {
        Self::caused_by_box(code, Box::new(source))
    }

    pub(super) fn caused_by_box(
        code: impl Into<ManagedJavaErrorKind>,
        source: Box<dyn std::error::Error + Send + Sync>,
    ) -> Self {
        let mut error = Self::new(code, &source);
        let cause: &(dyn std::error::Error + 'static) = source.as_ref();
        let mut nested = cause.source();
        // The diagnostic projection preserves nested IO/transport context; policy
        // below still examines typed causes, never this text.
        for _ in 0..32 {
            let Some(cause) = nested else { break };
            error.details.push_str(&format!("\nCaused by: {cause}"));
            nested = cause.source();
        }
        error.retryable = cause
            .downcast_ref::<crate::utils::download::error::DownloadError>()
            .is_some_and(|e| e.is_transient())
            || cause
                .downcast_ref::<ManagedJavaCatalogFailures>()
                .is_some_and(|failures| {
                    failures
                        .warnings
                        .iter()
                        .any(|warning| warning.error.retryable)
                })
            || cause
                .downcast_ref::<crate::utils::download::json::JsonRequestError>()
                .is_some_and(|e| e.is_transient())
            || cause
                .downcast_ref::<crate::utils::download::http::HttpError>()
                .is_some_and(|e| e.is_transient())
            || cause
                .downcast_ref::<std::io::Error>()
                .is_some_and(crate::utils::download::error::transient_io);
        if matches!(error.code, ManagedJavaErrorKind::Cancelled) {
            error.retryable = false;
        }
        error.source = Some(std::sync::Arc::from(source));
        error
    }

    pub fn diagnostic(&self) -> String {
        format!(
            "{}\nCode: {}\nStage: {:?}\n\n{}",
            self.message, self.code, self.stage, self.details
        )
    }
}

pub type Result<T> = std::result::Result<T, ManagedJavaError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CancelDisposition {
    Requested,
    AlreadyFinished,
}

#[cfg(test)]
mod error_tests {
    use super::*;
    use std::error::Error;

    #[test]
    fn retryability_uses_status_and_io_cause_not_domain_code_or_message() {
        for (status, retryable) in [(404, false), (429, true), (503, true)] {
            let error = ManagedJavaError::caused_by(
                ManagedJavaErrorKind::Download,
                crate::utils::download::error::DownloadError::Http {
                    status: surf::StatusCode::try_from(status).unwrap(),
                    url: "https://example.invalid".into(),
                },
            );
            assert_eq!(error.retryable, retryable);
            assert!(error
                .source()
                .unwrap()
                .downcast_ref::<crate::utils::download::error::DownloadError>()
                .is_some());
        }
        let denied = ManagedJavaError::caused_by(
            ManagedJavaErrorKind::Storage,
            std::io::Error::new(std::io::ErrorKind::PermissionDenied, "cancelled"),
        );
        assert!(!denied.retryable);
        assert_eq!(denied.code, ManagedJavaErrorKind::Storage);
        assert!(!ManagedJavaError::new(ManagedJavaErrorKind::Catalog, "timeout").retryable);
        let timeout = ManagedJavaError::caused_by(
            ManagedJavaErrorKind::Catalog,
            crate::utils::download::json::JsonRequestError::Timeout,
        );
        assert!(timeout.retryable);
    }

    #[test]
    fn durable_errors_round_trip_old_and_unknown_codes_without_fabricated_sources() {
        let old = serde_json::json!({"code":"future_code", "stage":null,"message":"legacy localized text", "details":"old diagnostic", "retryable":true});
        let error: ManagedJavaError = serde_json::from_value(old.clone()).unwrap();
        assert!(matches!(error.code, ManagedJavaErrorKind::Unknown(_)));
        assert!(error.source.is_none());
        assert_eq!(serde_json::to_value(error).unwrap(), old);
        let current = ManagedJavaError::caused_by(
            ManagedJavaErrorKind::Validation,
            std::io::Error::other("fixture"),
        );
        let loaded: ManagedJavaError =
            serde_json::from_slice(&serde_json::to_vec(&current).unwrap()).unwrap();
        assert_eq!(loaded, current);
        assert!(loaded.source.is_none());
    }
}
