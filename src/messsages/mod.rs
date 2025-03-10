use base::BaseMessage;
use serde::{Serialize, Deserialize};

pub mod info;
pub mod base;
pub mod progress;


// Errors
#[derive(Serialize, Deserialize, Debug)]
pub struct ErrorDetails {
    pub reason: String,
    pub suggestions: Vec<String>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct ErrorMessage {
    #[serde(flatten)]
    pub base: BaseMessage,

    pub details: ErrorDetails,
}
