use serde::{Serialize, Deserialize};

#[derive(Serialize, Deserialize, Debug)]
pub struct BaseMessage<'a> {
    pub message_id: String,
    pub timestamp: String,
    pub request_id: Option<&'a str>
}
