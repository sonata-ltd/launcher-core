use serde::{Serialize, Deserialize};

use super::base::BaseMessage;


// Progress Message
#[derive(Serialize, Deserialize, Debug)]
pub struct ProgressMessage<'a> {
    #[serde(flatten, borrow)]
    pub base: BaseMessage<'a>,

    pub data: ProgressData,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct ProgressData {
    pub stage: String,
    pub determinable: bool,
    pub progress: Option<usize>,
    pub max: usize,
    pub status: String,
    pub target_type: String,
    pub target: ProgressTarget,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(tag = "type")]
pub enum ProgressTarget {
    File {
        status: String,
        name: String,
        size_bytes: u64,
    },
    Dir {
        status: String,
        path: String,
    }
}


// Progress Targets List
#[derive(Serialize, Deserialize, Debug)]
pub struct ProgressTargetsList<'a> {
    #[serde(flatten, borrow)]
    pub base: BaseMessage<'a>,

    pub message_type: String,
    pub ids_list: Vec<String>,
}


// Progress Finish
#[derive(Serialize, Deserialize, Debug)]
pub struct ProgressFinishData {
    pub stage: String,
    pub status: String,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct ProgressFinishMessage<'a> {
    #[serde(flatten, borrow)]
    pub base: BaseMessage<'a>,

    pub message_type: String,
    pub data: ProgressFinishData,
}
