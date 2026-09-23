//! Pure lifecycle decision types and identities.

mod desired;
mod ids;
mod values;

pub use desired::{
    DesiredField, DesiredIntent, DispositionChange, LifecycleDesired, OutputProjection,
    OutputProjectionRemoval, ProjectionResult, Representative, SeatTarget, TopologyChangeClass,
};
pub use ids::{ClockProbeId, LifecycleEpochId, LifecycleTransitionId};
pub use values::{
    DeviceLifecycleState, Disposition, InvalidationReason, LifecycleEventId, LifecycleKind,
    Prerequisite, RecoveryId, TransitionTag, WorkTag,
};
