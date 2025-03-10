use serde::{Deserialize, Serialize};

use super::BaseMessage;


#[derive(Serialize, Deserialize, Debug)]
pub struct InfoMessage<'a> {
    #[serde(flatten, borrow)]
    pub base: BaseMessage<'a>,

    pub message: String
}
