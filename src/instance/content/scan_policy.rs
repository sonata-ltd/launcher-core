use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerificationMode {
    Fast,
    Full,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentityDecision {
    UseCachedIdentity,
    QueueBackgroundIndex,
    HashBeforeLaunch,
}

pub fn identity_decision(
    mode: VerificationMode,
    has_indexed_identity: bool,
    fingerprint_matches: bool,
    content_may_have_changed: bool,
) -> IdentityDecision {
    match mode {
        VerificationMode::Full => IdentityDecision::HashBeforeLaunch,
        VerificationMode::Fast
            if has_indexed_identity && fingerprint_matches && !content_may_have_changed =>
        {
            IdentityDecision::UseCachedIdentity
        }
        VerificationMode::Fast => IdentityDecision::QueueBackgroundIndex,
    }
}

#[derive(Default)]
pub struct DirtyGeneration {
    generation: AtomicU64,
}

impl DirtyGeneration {
    pub fn mark_changed(&self) {
        self.generation.fetch_add(1, Ordering::AcqRel);
    }

    pub fn current(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    pub fn changed_since(&self, started_generation: u64) -> bool {
        self.current() != started_generation
    }
}
