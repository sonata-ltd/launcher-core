use serde::{Deserialize, Serialize};

use crate::data::registry::operation::message::stage::OperationStage;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutputLine {
    /// Stable while a partial line is updated; increasing within a processor.
    pub id: u64,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessorOutput {
    pub stage: OperationStage,
    pub processor_index: usize,

    /// Increasing across the entire operation, including processor boundaries.
    pub revision: u64,

    /// Full chronological tail. An empty tail can announce a new processor.
    pub lines: Vec<OutputLine>,
}
