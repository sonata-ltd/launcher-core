//! DTO сценариев экземпляра. Здесь нет сетевых DTO провайдеров.
use crate::{instance::content::model::ProjectKey, instance::model::InstanceId};
use serde::{Deserialize, Serialize};
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
}

impl ContentJobSnapshot {
    pub fn active(&self) -> bool {
        matches!(
            self.phase,
            ContentPhase::Downloading | ContentPhase::Publishing
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct PublishedFile {
    pub name: String,
    pub sha256: String,
    pub object_id: i64,
    pub provider: String,
    pub project_id: String,
    pub release_id: String,
    pub placement: String,
}
