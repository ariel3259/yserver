//! Pure lifecycle decision types and identities.

mod ids;
mod values;

pub use ids::{ClockProbeId, LifecycleEpochId, LifecycleTransitionId};
pub use values::{
    DeviceLifecycleState, Disposition, InvalidationReason, LifecycleEventId, LifecycleKind,
    Prerequisite, RecoveryId, TransitionTag, WorkTag,
};
