use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum LifeCycle {
    Started,
    InProgress,
    Completed,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize, Hash)]
pub enum Outcome {
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ProgressUnit {
    Bytes,
    Items,
    Percent,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Progress {
    Determinable { current: usize, total: usize },
    Indeterminable,
}
