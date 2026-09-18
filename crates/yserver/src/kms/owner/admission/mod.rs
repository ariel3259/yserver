use std::collections::BTreeSet;

pub type CrtcId = u32;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum AdmissionError {
    #[error("generation {offered} is not newer than queued generation {queued}")]
    StaleGeneration { queued: u64, offered: u64 },
    #[error("an unflip barrier is pending")]
    UnflipPending,
    #[error("the CRTC set is empty")]
    EmptyCrtcSet,
}

mod decide;
mod intents;
mod snapshot;

pub use decide::{AdmissionDecision, Admitted, Tier};
pub use intents::{
    Admission, ComposedIntent, DirectSuccessor, PrimaryOrdinal, QueuedDirect, UnflipBarrier,
};
pub use snapshot::{IntentKey, Readiness, ReadinessSnapshot, WaitReason};

#[doc(hidden)]
pub fn crtcs(ids: &[CrtcId]) -> BTreeSet<CrtcId> {
    ids.iter().copied().collect()
}

#[cfg(test)]
mod tests;
