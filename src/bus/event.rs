use crate::{
    data::registry::operation::message::OperationMessage,
    instance::{event::InstanceEvent, options::model::OptionUpdateMessage, scan::ScanMessage},
    java::model::JavaEvent,
};

#[derive(Debug, Clone)]
pub enum CoreEvent {
    Operation(OperationMessage),
    Option(OptionUpdateMessage),
    Scan(ScanMessage),
    Instance(InstanceEvent),
    Java(JavaEvent),
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

impl From<InstanceEvent> for CoreEvent {
    fn from(value: InstanceEvent) -> Self {
        CoreEvent::Instance(value)
    }
}

impl From<JavaEvent> for CoreEvent {
    fn from(value: JavaEvent) -> Self {
        CoreEvent::Java(value)
    }
}
