//! DTO сценариев экземпляра. Здесь нет сетевых DTO провайдеров.
use crate::{instance::content::model::ProjectKey, instance::model::InstanceId};
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct InstallPreview {
    pub id: Uuid,
    pub instance_id: InstanceId,
    pub items: Vec<PreviewItem>,
    pub download_bytes: u64,
}

#[derive(Debug, Clone)]
pub struct PreviewItem {
    pub title: String,
    pub file_name: String,
    pub project: ProjectKey,
    pub is_root: bool,
    pub size_bytes: u64,
    pub already_installed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentPhase {
    Downloading,
    Publishing,
    Completed,
    Cancelled,
    Failed,
}

#[derive(Debug, Clone)]
pub struct ContentJobSnapshot {
    pub operation_id: crate::data::registry::operation::message::OperationId,
    pub id: Uuid,
    pub instance_id: InstanceId,
    pub phase: ContentPhase,
    pub completed_files: usize,
    pub total_files: usize,
    pub error: Option<String>,
    pub finished_at: Option<chrono::DateTime<chrono::Utc>>,
}

impl ContentJobSnapshot {
    pub fn outcome(&self) -> Option<crate::data::registry::operation::message::status::Outcome> {
        use crate::data::registry::operation::message::status::Outcome;
        match self.phase {
            ContentPhase::Completed => Some(Outcome::Completed),
            ContentPhase::Cancelled => Some(Outcome::Cancelled),
            ContentPhase::Failed => Some(Outcome::Failed),
            ContentPhase::Downloading | ContentPhase::Publishing => None,
        }
    }
    pub fn active(&self) -> bool {
        matches!(
            self.phase,
            ContentPhase::Downloading | ContentPhase::Publishing
        )
    }
}
