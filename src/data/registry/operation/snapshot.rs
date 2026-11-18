use serde::{Deserialize, Serialize};

use crate::data::registry::operation::message::{
    event::{OperationEvent, OperationUpdate},
    stage::{OperationStage, StageSnapshot, StageStatus},
    status::{LifeCycle, Outcome, Progress},
    OperationId,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperationSnapshot {
    pub id: OperationId,
    pub name: String,
    #[serde(default)]
    pub subject: Option<super::subject::OperationSubject>,

    pub stage: Option<OperationStage>,
    pub stages: Vec<StageSnapshot>,

    pub status: LifeCycle,
    pub progress: Progress,

    pub message: Option<String>,
}

impl OperationSnapshot {
    pub(super) fn apply(&mut self, event: &OperationEvent) {
        match event {
            OperationEvent::Start(start) => {
                self.status = LifeCycle::Started;

                self.stages = start
                    .stages
                    .iter()
                    .copied()
                    .map(|stage| StageSnapshot {
                        stage,
                        status: StageStatus::Pending,
                        progress: Progress::Indeterminable,
                        reason: None,
                    })
                    .collect();
            }
            OperationEvent::Update(OperationUpdate::Progress {
                stage,
                status,
                progress,
                ..
            }) => {
                self.stage = Some(stage.clone());
                self.status = status.clone();
                self.progress = progress.clone();

                if let Some(entry) = self.stages.iter_mut().find(|entry| entry.stage == *stage) {
                    entry.status = StageStatus::Running;
                    entry.progress = progress.clone();
                    entry.reason = None;
                }
            }
            OperationEvent::Update(OperationUpdate::Completed(res)) => {
                self.stage = Some(res.stage.clone());

                if matches!(res.status, Outcome::Failed) {
                    self.status = LifeCycle::Failed;
                }

                if let Some(entry) = self
                    .stages
                    .iter_mut()
                    .find(|entry| entry.stage == res.stage)
                {
                    entry.status = match res.status {
                        Outcome::Completed => StageStatus::Completed,
                        Outcome::Failed => StageStatus::Failed,
                        Outcome::Cancelled => StageStatus::Cancelled,
                    };
                }
            }
            OperationEvent::Update(OperationUpdate::Skipped { stage, reason }) => {
                if let Some(entry) = self.stages.iter_mut().find(|entry| entry.stage == *stage) {
                    entry.status = StageStatus::Skipped;
                    entry.reason = reason.clone();
                }
            }
            OperationEvent::Update(OperationUpdate::Output(_)) => {}
            OperationEvent::Finish(finish) => {
                for entry in &mut self.stages {
                    if matches!(entry.status, StageStatus::Running) {
                        entry.status = match finish.outcome {
                            Outcome::Failed => StageStatus::Failed,
                            Outcome::Cancelled => StageStatus::Cancelled,
                            Outcome::Completed => continue,
                        };
                    }
                }
                self.status = match finish.outcome {
                    Outcome::Completed => LifeCycle::Completed,
                    Outcome::Failed => LifeCycle::Failed,
                    // Preserve the public lifecycle enum; exact cancellation is
                    // available in the terminal outcome, not confused with success.
                    Outcome::Cancelled => LifeCycle::Failed,
                };
            }
        }
    }
}
