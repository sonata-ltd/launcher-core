use base::BaseMessage;
use serde::{Serialize, Deserialize};

pub mod info;
pub mod base;
pub mod progress;
pub mod scan;


// Errors
#[derive(Serialize, Deserialize, Debug)]
pub struct ErrorDetails {
    pub reason: String,
    pub suggestions: Vec<String>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct ErrorMessage<'a> {
    #[serde(flatten, borrow)]
    pub base: BaseMessage<'a>,

    pub details: ErrorDetails,
}
