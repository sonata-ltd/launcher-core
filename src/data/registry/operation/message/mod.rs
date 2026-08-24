use serde::{Deserialize, Serialize};

use crate::data::registry::operation::message::event::OperationEvent;

pub mod event;
pub mod process;
pub mod stage;
pub mod status;
pub mod target;

#[derive(Debug, Copy, Clone, Serialize, Deserialize, Eq, PartialEq, Hash)]
pub struct OperationId(u32);

impl OperationId {
    pub(super) fn new(raw: u32) -> Self {
        Self(raw)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperationMessage {
    pub operation_id: OperationId,
    pub data: OperationEvent,
}
