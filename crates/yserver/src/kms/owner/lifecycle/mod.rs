//! Pure lifecycle decision types and identities.

mod arbiter;
mod coordinator;
mod desired;
mod ids;
mod recovery;
mod values;

pub use arbiter::{
    ArbiterInput, CommitProgress, LifecycleAction, LifecycleArbiter, LifecycleCommitOutcome,
    LifecycleReceipt, LifecycleReceiptResult, LifecycleTransition, LifecycleTransitionPhase,
    RecoveryAttemptOutcome,
};
pub use coordinator::{
    CoordinatorDispatch, CoordinatorError, CoordinatorOutputAddition, LifecycleCoordinator,
};
pub use desired::{
    DesiredField, DesiredIntent, DispositionChange, LifecycleDesired, OutputProjection,
    OutputProjectionRemoval, ProjectionResult, Representative, SeatTarget, TopologyChangeClass,
};
pub use ids::{ClockProbeId, LifecycleEpochId, LifecycleTransitionId};
pub use recovery::{
    CompletionUnknownRow, CompletionUnknownRowKind, DpmsTarget, EventFate, IncidentInvalidation,
    IncidentOrigin, IncidentResolution, IncidentSeed, LogicalUnknownActions, PhysicalBarriers,
    PhysicalUnknownOutcome, RecoveryAttemptBudget, RecoveryAttemptTrigger, RecoveryFate,
    RecoveryIdAllocator, RecoveryIncident, RecoveryIncidentState, RecoveryResolution,
    RecoveryWinner, TableFOutcome, TableUOutcome, table_f, table_u,
};
pub use values::{
    DeviceLifecycleState, Disposition, InvalidationReason, LifecycleEventId, LifecycleKind,
    Prerequisite, RecoveryId, TransitionTag, WorkTag,
};
