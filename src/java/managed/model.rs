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
        let os = match std::env::consts::OS {
            "linux" if cfg!(target_env = "musl") => "alpine-linux",
            "linux" => "linux",
            "windows" => "windows",
            "macos" => "mac",
            other => return Err(ManagedJavaError::new("unsupported_platform", other)),
        };
        let arch = match super::super::select::host_arch() {
            "x86_64" => "x64",
            "x86" => "x32",
            "aarch64" => "aarch64",
            "arm" => "arm",
            other => return Err(ManagedJavaError::new("unsupported_platform", other)),
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

#[derive(Debug, Clone, Serialize, Deserialize, thiserror::Error, PartialEq, Eq)]
#[error("{message}")]
pub struct ManagedJavaError {
    pub code: String,
    pub stage: Option<ManagedJavaStage>,
    pub message: String,
    pub details: String,
    pub retryable: bool,
}

impl ManagedJavaError {
    pub fn new(code: &str, details: impl std::fmt::Display) -> Self {
        Self {
            code: code.into(),
            stage: None,
            message: match code {
                "cancelled" => "Installation cancelled",
                "catalog" => "Failed to load available Java runtimes",
                "download" => "Failed to download Java",
                "checksum" => "Java archive failed integrity verification",
                "extract" => "Failed to extract Java",
                "validation" => "Java runtime failed validation",
                "storage" => "Failed to save Java installation",
                "registration" => "Failed to register Java runtime",
                "busy" => "This Java package already has an installation",
                "not_found" => "Java installation was not found",
                "unsupported_platform" => "No Java runtime is available for this platform",
                _ => "Managed Java operation failed",
            }
            .into(),
            details: details.to_string(),
            retryable: matches!(
                code,
                "download" | "catalog" | "storage" | "validation" | "registration"
            ),
        }
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
