use crate::instance::model::InstanceId;

#[derive(Debug, Clone)]
pub enum InstanceEvent {
    PreparingStart(InstanceId),
    Started { id: InstanceId, pid: u32 },
    LaunchFailed(InstanceId),
    Exited { id: InstanceId, code: Option<i32> },
    SettingsChanged(InstanceId),
}
