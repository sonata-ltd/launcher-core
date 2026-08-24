use serde::{Deserialize, Serialize};

use crate::data::registry::operation::message::{
    event::{OperationEvent, OperationUpdate},
    stage::OperationStage,
    status::{LifeCycle, Outcome, Progress},
    OperationId,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperationSnapshot {
    pub id: OperationId,
    pub name: String,

    pub stage: Option<OperationStage>,

    pub status: LifeCycle,
    pub progress: Progress,

    pub message: Option<String>,
}

impl OperationSnapshot {
    pub(super) fn apply(&mut self, event: &OperationEvent) {
        match event {
            OperationEvent::Start(_) => {
                self.status = LifeCycle::Started;
            }
            OperationEvent::Update(OperationUpdate::Progress {
                stage,
                status,
                progress,
                ..
            }) => {
                self.stage = Some(stage.clone());
                self.status = status.clone();
                self.progress = progress.clone()
            }
            OperationEvent::Update(OperationUpdate::Completed(res)) => {
                self.stage = Some(res.stage.clone());

                if matches!(res.status, Outcome::Failed) {
                    self.status = LifeCycle::Failed;
                }
            }
            OperationEvent::Finish(_) => {}
        }
    }
}
