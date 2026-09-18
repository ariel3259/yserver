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
    #[error("an admission token is already outstanding")]
    AlreadyLocked,
    #[error("the admission decision no longer matches the queued state")]
    DecisionMismatch,
    #[error("the admission token belongs to another decider or is stale")]
    TokenMismatch,
    #[error("the maintenance admission ticket counter overflowed")]
    TicketOverflow,
}

mod decide;
mod intents;
mod snapshot;
mod token;

pub use decide::{AdmissionDecision, Admitted, CarriedMaintenance, Tier};
pub use intents::{
    Admission, AdmissionTicket, ComposedIntent, DirectSuccessor, MaintenanceClass,
    MaintenanceIntent, MaintenanceKey, PrimaryOrdinal, QueuedDirect, Reentry, ReentryKind,
    UnflipBarrier,
};
pub use snapshot::{IntentKey, Readiness, ReadinessSnapshot, WaitReason};
pub use token::{AdmissionToken, Confirmed};

#[doc(hidden)]
pub fn crtcs(ids: &[CrtcId]) -> BTreeSet<CrtcId> {
    ids.iter().copied().collect()
}

#[cfg(test)]
mod tests;
