use crate::{
    data::registry::operation::message::OperationMessage,
    instance::{options::model::OptionUpdateMessage, scan::ScanMessage},
};

#[derive(Debug, Clone)]
pub enum CoreEvent {
    Operation(OperationMessage),
    Option(OptionUpdateMessage),
    Scan(ScanMessage),
}

impl From<OperationMessage> for CoreEvent {
    fn from(value: OperationMessage) -> Self {
        CoreEvent::Operation(value)
    }
}

impl From<OptionUpdateMessage> for CoreEvent {
    fn from(value: OptionUpdateMessage) -> Self {
        CoreEvent::Option(value)
    }
}

impl From<ScanMessage> for CoreEvent {
    fn from(value: ScanMessage) -> Self {
        CoreEvent::Scan(value)
    }
}
