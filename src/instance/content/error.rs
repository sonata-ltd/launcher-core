use thiserror::Error;

#[derive(Debug, Error)]
pub enum ContentError {
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
    #[error("invalid content journal operation id {0:?}")]
    InvalidJournalId(String),
    #[error("CurseForge недоступен: задайте CURSEFORGE_API_KEY в окружении лаунчера")]
    MissingCurseForgeKey,
    #[error("Каталог вернул HTTP {0}; повторите запрос позднее")]
    Http(u16),
    #[error("{0}")]
    Transport(String),
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

pub type Result<T> = std::result::Result<T, ContentError>;
