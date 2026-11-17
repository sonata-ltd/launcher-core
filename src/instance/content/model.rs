use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, sqlx::Type)]
#[sqlx(rename_all = "lowercase")]
pub enum Provider {
    Modrinth,
    CurseForge,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ProjectKey {
    pub provider: Provider,
    pub project_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ReleaseKey {
    pub project: ProjectKey,
    pub release_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DependencyKind {
    Required,
    Optional,
    Incompatible,
    Embedded,
    Tool,
    Include,
    Unknown(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Dependency {
    pub project: ProjectKey,
    pub exact_release: Option<String>,
    pub kind: DependencyKind,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Candidate {
    pub key: ReleaseKey,
    pub dependencies: Vec<Dependency>,
}

/// Назначение размещения, а не свойство байтов объекта CAS.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ContentKind {
    Mod,
    ResourcePack,
    ShaderPack,
}
