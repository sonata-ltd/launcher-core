use serde::{Deserialize, Serialize};

use crate::instance::model::InstanceId;

/// Typed observer correlation, never inferred from a human-readable name.
/// Managed installation UUIDs are durable subjects; operation IDs are local
/// to this session. Neither identity is a retention pin or an admission token.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OperationSubject {
    InstanceInstall {
        instance: InstanceId,
    },
    ContentInstall {
        instance: InstanceId,
        installation: uuid::Uuid,
    },
    ManagedInstall {
        installation: uuid::Uuid,
    },
    ManagedValidation {
        installation: uuid::Uuid,
    },
}
