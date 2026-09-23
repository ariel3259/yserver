//! Pure completion-loss and recovery-fate decisions from C.0 §10 and REC-6.

use super::{
    Disposition, InvalidationReason, LifecycleEventId, LifecycleKind, LifecycleTransitionId,
    RecoveryId,
};

/// Whether the current projected DPMS level requests power on or off.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub enum DpmsTarget {
    On,
    Off,
}

/// Convert an X11 DPMS level to the binary KMS power target.
///
/// Level 0 means On; Standby, Suspend, and Off (levels 1–3) all mean Off.
pub const fn dpms_target_for_level(level: u8) -> Option<DpmsTarget> {
    match level {
        0 => Some(DpmsTarget::On),
        1..=3 => Some(DpmsTarget::Off),
        _ => None,
    }
}

/// State of one incident's sole automatic recovery attempt.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub enum RecoveryIncidentState {
    Active,
    Paused,
    Attempting,
    RecoveryFailed,
}

impl RecoveryIncidentState {
    /// Every state used to generate recovery-table cases.
    pub const ALL: [Self; 4] = [
        Self::Active,
        Self::Paused,
        Self::Attempting,
        Self::RecoveryFailed,
    ];
}

/// Whether the incident's one attempt remains available.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub enum RecoveryAttemptBudget {
    Available,
    Spent,
}

/// Only the post-reap continuation may start the sole automatic attempt.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub enum RecoveryAttemptTrigger {
    AfterReap,
    Timer,
    Dpms,
    ClientTraffic,
    QueuedIntent,
}

impl RecoveryAttemptTrigger {
    pub const ALL: [Self; 5] = [
        Self::AfterReap,
        Self::Timer,
        Self::Dpms,
        Self::ClientTraffic,
        Self::QueuedIntent,
    ];
}

/// How a recovery incident obtained authority.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub enum IncidentOrigin {
    /// Completion loss reported by the coordinator; its event stays pending
    /// until this incident resolves.
    CompletionLoss { representative: LifecycleEventId },
    /// Recovery authorized by a REC-6 external boundary; it has no event
    /// representative and is recorded on that boundary transition.
    Boundary,
}

/// An id-free request to create one incident. The per-device allocator
/// materializes this seed; pure table functions never allocate identities.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub struct IncidentSeed {
    pub origin: IncidentOrigin,
    pub initial_state: RecoveryIncidentState,
}

impl IncidentSeed {
    pub const fn completion_loss(
        representative: LifecycleEventId,
        initial_state: RecoveryIncidentState,
    ) -> Self {
        Self {
            origin: IncidentOrigin::CompletionLoss { representative },
            initial_state,
        }
    }

    pub const fn boundary() -> Self {
        Self {
            origin: IncidentOrigin::Boundary,
            initial_state: RecoveryIncidentState::Active,
        }
    }
}

/// One device's recovery incident and its bounded attempt budget.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub struct RecoveryIncident {
    id: RecoveryId,
    origin: IncidentOrigin,
    state: RecoveryIncidentState,
    budget: RecoveryAttemptBudget,
}

impl RecoveryIncident {
    const fn from_seed(id: RecoveryId, seed: IncidentSeed) -> Self {
        let budget = if matches!(seed.initial_state, RecoveryIncidentState::RecoveryFailed) {
            RecoveryAttemptBudget::Spent
        } else {
            RecoveryAttemptBudget::Available
        };
        Self {
            id,
            origin: seed.origin,
            state: seed.initial_state,
            budget,
        }
    }

    const fn failed_boundary(id: RecoveryId) -> Self {
        Self {
            id,
            origin: IncidentOrigin::Boundary,
            state: RecoveryIncidentState::RecoveryFailed,
            budget: RecoveryAttemptBudget::Spent,
        }
    }

    pub const fn id(self) -> RecoveryId {
        self.id
    }

    pub const fn origin(self) -> IncidentOrigin {
        self.origin
    }

    pub const fn state(self) -> RecoveryIncidentState {
        self.state
    }

    pub const fn budget(self) -> RecoveryAttemptBudget {
        self.budget
    }

    pub const fn representative(self) -> Option<LifecycleEventId> {
        match self.origin {
            IncidentOrigin::CompletionLoss { representative } => Some(representative),
            IncidentOrigin::Boundary => None,
        }
    }

    /// Claim the sole attempt. A paused or spent incident cannot start one.
    pub fn begin_attempt(&mut self) -> bool {
        if self.state != RecoveryIncidentState::Active
            || self.budget != RecoveryAttemptBudget::Available
        {
            return false;
        }
        self.state = RecoveryIncidentState::Attempting;
        self.budget = RecoveryAttemptBudget::Spent;
        true
    }

    /// Reject retry authority from timers, DPMS, client work, or queued
    /// intents. They may not spend or replenish the incident's sole attempt.
    pub fn request_attempt(&mut self, trigger: RecoveryAttemptTrigger) -> bool {
        trigger == RecoveryAttemptTrigger::AfterReap && self.begin_attempt()
    }

    /// Pause without changing the budget. If an attempt had already started,
    /// resuming returns to `Attempting` so it cannot be started a second time.
    pub fn pause(&mut self) -> bool {
        if !matches!(
            self.state,
            RecoveryIncidentState::Active | RecoveryIncidentState::Attempting
        ) {
            return false;
        }
        self.state = RecoveryIncidentState::Paused;
        true
    }

    /// Resume at the logical DPMS-on boundary, preserving the same id/budget.
    pub fn resume(&mut self) -> bool {
        if self.state != RecoveryIncidentState::Paused {
            return false;
        }
        self.state = match self.budget {
            RecoveryAttemptBudget::Available => RecoveryIncidentState::Active,
            RecoveryAttemptBudget::Spent => RecoveryIncidentState::Attempting,
        };
        true
    }

    /// Resolve a qualified install, or retain a terminal failed record after
    /// the sole attempt fails or completes with unknown status.
    pub fn resolve(self, result: RecoveryResolution) -> IncidentResolution {
        if self.state != RecoveryIncidentState::Attempting {
            return IncidentResolution {
                id: self.id,
                incident: Some(self),
                representative_disposition: None,
            };
        }
        match result {
            RecoveryResolution::Qualified(transition) => IncidentResolution {
                id: self.id,
                incident: None,
                representative_disposition: self
                    .representative()
                    .map(|event_id| (event_id, Disposition::Applied(transition))),
            },
            RecoveryResolution::Failed => {
                let failed = Self {
                    state: RecoveryIncidentState::RecoveryFailed,
                    budget: RecoveryAttemptBudget::Spent,
                    ..self
                };
                IncidentResolution {
                    id: self.id,
                    incident: Some(failed),
                    representative_disposition: self.representative().map(|event_id| {
                        (
                            event_id,
                            Disposition::Invalidated(InvalidationReason::RecoveryFailed),
                        )
                    }),
                }
            }
        }
    }
}

/// Terminal result of the incident's single attempt.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub enum RecoveryResolution {
    Qualified(LifecycleTransitionId),
    Failed,
}

/// Result of resolving one incident; a successful incident is consumed, while
/// a failed record remains so REC-1 cannot authorize another attempt.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub struct IncidentResolution {
    pub id: RecoveryId,
    pub incident: Option<RecoveryIncident>,
    pub representative_disposition: Option<(LifecycleEventId, Disposition)>,
}

/// Checked, per-device allocation for recovery incident identities.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub struct RecoveryIdAllocator {
    next: Option<RecoveryId>,
}

impl Default for RecoveryIdAllocator {
    fn default() -> Self {
        Self::new()
    }
}

impl RecoveryIdAllocator {
    pub const fn new() -> Self {
        Self {
            next: Some(RecoveryId::first()),
        }
    }

    /// Allocate one checked identity. The final representable id may be used;
    /// the following request returns `None` rather than wrapping.
    pub fn allocate(&mut self) -> Option<RecoveryId> {
        let id = self.next?;
        self.next = id.checked_next();
        Some(id)
    }

    /// Materialize a table-produced seed without giving the arbiter its own
    /// allocation policy.
    pub fn allocate_incident(&mut self, seed: IncidentSeed) -> Option<RecoveryIncident> {
        self.allocate()
            .map(|id| RecoveryIncident::from_seed(id, seed))
    }

    #[cfg(test)]
    fn with_next(next: RecoveryId) -> Self {
        Self { next: Some(next) }
    }
}

/// Active transition row at the instant a completion loss is observed.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub enum CompletionUnknownRowKind {
    NormalLive,
    VTRelease,
    DeviceRemoved,
    Shutdown,
    DeviceAddedOrReplaced,
    VTAcquire,
    AdministrativeReprobe,
    IdentityChangingHotplug,
    TopologyRebuild,
    DPMS,
}

impl CompletionUnknownRowKind {
    /// All §10 rows, in their documented order, for generated tests.
    pub const ALL: [Self; 10] = [
        Self::NormalLive,
        Self::VTRelease,
        Self::DeviceRemoved,
        Self::Shutdown,
        Self::DeviceAddedOrReplaced,
        Self::VTAcquire,
        Self::AdministrativeReprobe,
        Self::IdentityChangingHotplug,
        Self::TopologyRebuild,
        Self::DPMS,
    ];
}

/// A §10 row. Boundary variants require their already allocated fresh ID, so
/// an unknown completion during that attempt cannot be represented without it.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub enum CompletionUnknownRow {
    NormalLive,
    VTRelease,
    DeviceRemoved,
    Shutdown,
    DeviceAddedOrReplaced { recovery_id: RecoveryId },
    VTAcquire { recovery_id: RecoveryId },
    AdministrativeReprobe { recovery_id: RecoveryId },
    IdentityChangingHotplug { recovery_id: RecoveryId },
    TopologyRebuild,
    DPMS { target: DpmsTarget },
}

impl CompletionUnknownRow {
    pub const fn kind(self) -> CompletionUnknownRowKind {
        match self {
            Self::NormalLive => CompletionUnknownRowKind::NormalLive,
            Self::VTRelease => CompletionUnknownRowKind::VTRelease,
            Self::DeviceRemoved => CompletionUnknownRowKind::DeviceRemoved,
            Self::Shutdown => CompletionUnknownRowKind::Shutdown,
            Self::DeviceAddedOrReplaced { .. } => CompletionUnknownRowKind::DeviceAddedOrReplaced,
            Self::VTAcquire { .. } => CompletionUnknownRowKind::VTAcquire,
            Self::AdministrativeReprobe { .. } => CompletionUnknownRowKind::AdministrativeReprobe,
            Self::IdentityChangingHotplug { .. } => {
                CompletionUnknownRowKind::IdentityChangingHotplug
            }
            Self::TopologyRebuild => CompletionUnknownRowKind::TopologyRebuild,
            Self::DPMS { .. } => CompletionUnknownRowKind::DPMS,
        }
    }
}

/// Whether the completion-loss event remains pending as a representative or
/// receives its terminal REC-5 disposition immediately.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub enum EventFate {
    PendingRepresentative(LifecycleEventId),
    Terminal {
        event_id: LifecycleEventId,
        disposition: Disposition,
    },
}

/// Immediate logical obligations from Table U. Physical work is kept in a
/// separate result so callers cannot accidentally gate logical progress on a
/// reap or fd-family barrier.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub struct LogicalUnknownActions {
    pub stop_admission: bool,
    pub withdraw_protocol_work: bool,
    pub release_seat: bool,
    pub revoke_readiness: bool,
    pub apply_dpms_target: bool,
    pub poison_device: bool,
    pub terminalize_rebuild: bool,
    pub event: EventFate,
}

/// Physical obligations from Table U. Variants name only work that must wait
/// for the stated barrier or can proceed without another KMS mutation.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub enum PhysicalUnknownOutcome {
    QuarantineReapCloseThenAttemptOnce,
    QuarantineAwaitLifecycleTeardown,
    QuarantineReapCloseWithoutAttempt,
    TransferQuarantineToRebuildThenQualifiedInstall,
    QuarantineReapCloseThenAttemptOnceAndTerminalizeRebuild,
    QuarantineBoundaryAttemptFailed {
        recovery_id: RecoveryId,
    },
    QuarantineDpmsWithoutKmsMutation {
        attempt_after_reap: bool,
        paused_until_dpms_on: bool,
    },
    ExistingAttemptFailedWithoutRetry,
    ExistingFailureRemainsWithoutRetry,
}

/// Shared executor and resource barriers required for every unknown commit.
/// The returned value is separate from immediate logical obligations.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub struct PhysicalBarriers {
    pub quarantine_both_possible_states: bool,
    pub stop_alias_creation: bool,
    pub request_executor_and_helper_termination: bool,
    pub reap_every_lease: bool,
    pub close_complete_fd_family_after_reap: bool,
}

impl PhysicalBarriers {
    const UNKNOWN_COMMIT: Self = Self {
        quarantine_both_possible_states: true,
        stop_alias_creation: true,
        request_executor_and_helper_termination: true,
        reap_every_lease: true,
        close_complete_fd_family_after_reap: true,
    };
}

/// A terminal invalidation of a previous incident, attributed to its named
/// REC-6 reason.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub struct IncidentInvalidation {
    pub recovery_id: RecoveryId,
    pub reason: InvalidationReason,
}

/// Explicit incident and event effects shared by both tables.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash, Default)]
pub struct RecoveryFate {
    /// The same current incident, possibly paused, resumed, or failed.
    pub current_incident: Option<RecoveryIncident>,
    /// An id-free seed; its owner materializes it with `RecoveryIdAllocator`.
    pub new_incident: Option<IncidentSeed>,
    /// An old incident invalidated by a REC-6 boundary.
    pub invalidated_incident: Option<IncidentInvalidation>,
    /// True only for REC-6's same-identity topology transfer.
    pub transferred: bool,
    /// U-1b disposition for a loss incident's representative, if it resolved.
    pub representative_disposition: Option<(LifecycleEventId, Disposition)>,
    /// Terminal disposition for an additional lifecycle/recovery event.
    pub event_disposition: Option<(LifecycleEventId, Disposition)>,
}

/// The complete pure Table U result.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub struct TableUOutcome {
    pub logical: LogicalUnknownActions,
    pub physical: PhysicalUnknownOutcome,
    pub physical_barriers: PhysicalBarriers,
    pub recovery: RecoveryFate,
}

/// Lifecycle kind winning under REC-6. `recovery_required` is the boundary
/// transition's decision that a fresh install/attempt is required.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub struct RecoveryWinner {
    pub kind: LifecycleKind,
    pub dpms_target: DpmsTarget,
    pub recovery_required: bool,
}

impl RecoveryWinner {
    pub const fn new(kind: LifecycleKind) -> Self {
        Self {
            kind,
            dpms_target: DpmsTarget::On,
            recovery_required: false,
        }
    }

    pub const fn with_dpms_target(mut self, target: DpmsTarget) -> Self {
        self.dpms_target = target;
        self
    }

    pub const fn requiring_recovery(mut self) -> Self {
        self.recovery_required = true;
        self
    }
}

/// Complete Table F outcome. `new_incident` asks the per-device allocator for
/// exactly one fresh ID; boundary seeds intentionally have no event rep.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub struct TableFOutcome {
    pub fate: RecoveryFate,
    pub incident_state: Option<RecoveryIncidentState>,
    pub allocate_fresh_id: bool,
}

/// C.0 §10: select the completion-loss outcome by the row active when the loss
/// is observed, regardless of whether the lost work was ordinary or owned by
/// that transition.
pub fn table_u(
    row: CompletionUnknownRow,
    event_id: LifecycleEventId,
    existing: Option<RecoveryIncident>,
) -> TableUOutcome {
    match row {
        CompletionUnknownRow::NormalLive => normal_live(event_id, existing, false),
        CompletionUnknownRow::VTRelease => teardown(
            event_id,
            existing,
            InvalidationReason::VTRelease,
            LogicalUnknownActions {
                stop_admission: true,
                withdraw_protocol_work: false,
                release_seat: true,
                revoke_readiness: true,
                apply_dpms_target: false,
                poison_device: false,
                terminalize_rebuild: false,
                event: terminal_event(event_id, InvalidationReason::VTRelease),
            },
        ),
        CompletionUnknownRow::DeviceRemoved => teardown(
            event_id,
            existing,
            InvalidationReason::DeviceRemoved,
            LogicalUnknownActions {
                stop_admission: true,
                withdraw_protocol_work: true,
                release_seat: false,
                revoke_readiness: true,
                apply_dpms_target: false,
                poison_device: false,
                terminalize_rebuild: false,
                event: terminal_event(event_id, InvalidationReason::DeviceRemoved),
            },
        ),
        CompletionUnknownRow::Shutdown => teardown(
            event_id,
            existing,
            InvalidationReason::Shutdown,
            LogicalUnknownActions {
                stop_admission: true,
                withdraw_protocol_work: true,
                release_seat: true,
                revoke_readiness: true,
                apply_dpms_target: false,
                poison_device: false,
                terminalize_rebuild: false,
                event: terminal_event(event_id, InvalidationReason::Shutdown),
            },
        ),
        CompletionUnknownRow::TopologyRebuild => match existing {
            Some(incident) if incident.state == RecoveryIncidentState::Attempting => {
                let resolution = incident.resolve(RecoveryResolution::Failed);
                TableUOutcome {
                    logical: LogicalUnknownActions {
                        stop_admission: true,
                        withdraw_protocol_work: false,
                        release_seat: false,
                        revoke_readiness: true,
                        apply_dpms_target: false,
                        poison_device: true,
                        terminalize_rebuild: true,
                        event: event_fate_for(
                            event_id,
                            incident,
                            InvalidationReason::RecoveryFailed,
                        ),
                    },
                    physical: PhysicalUnknownOutcome::ExistingAttemptFailedWithoutRetry,
                    physical_barriers: PhysicalBarriers::UNKNOWN_COMMIT,
                    recovery: RecoveryFate {
                        current_incident: resolution.incident,
                        representative_disposition: resolution.representative_disposition,
                        event_disposition: event_fate_disposition(event_id, incident),
                        ..RecoveryFate::default()
                    },
                }
            }
            Some(incident) if incident.state != RecoveryIncidentState::RecoveryFailed => {
                let fate = absorbed_fate(event_id, incident);
                TableUOutcome {
                    logical: LogicalUnknownActions {
                        stop_admission: true,
                        withdraw_protocol_work: false,
                        release_seat: false,
                        revoke_readiness: true,
                        apply_dpms_target: false,
                        poison_device: true,
                        terminalize_rebuild: false,
                        event: event_fate_for(
                            event_id,
                            incident,
                            InvalidationReason::RecoveryFailed,
                        ),
                    },
                    physical:
                        PhysicalUnknownOutcome::TransferQuarantineToRebuildThenQualifiedInstall,
                    physical_barriers: PhysicalBarriers::UNKNOWN_COMMIT,
                    recovery: RecoveryFate {
                        current_incident: Some(incident),
                        transferred: true,
                        ..fate
                    },
                }
            }
            Some(incident) => TableUOutcome {
                logical: poison_event(
                    event_id,
                    terminal_event(event_id, InvalidationReason::RecoveryFailed),
                ),
                physical: PhysicalUnknownOutcome::ExistingFailureRemainsWithoutRetry,
                physical_barriers: PhysicalBarriers::UNKNOWN_COMMIT,
                recovery: RecoveryFate {
                    current_incident: Some(incident),
                    event_disposition: Some((
                        event_id,
                        Disposition::Invalidated(InvalidationReason::RecoveryFailed),
                    )),
                    ..RecoveryFate::default()
                },
            },
            None => normal_live(event_id, None, true),
        },
        CompletionUnknownRow::DPMS { target } => dpms_unknown(event_id, target, existing),
        CompletionUnknownRow::DeviceAddedOrReplaced { recovery_id } => boundary_unknown(
            event_id,
            recovery_id,
            InvalidationReason::DeviceAddedOrReplaced,
            existing,
        ),
        CompletionUnknownRow::VTAcquire { recovery_id } => boundary_unknown(
            event_id,
            recovery_id,
            InvalidationReason::VTAcquire,
            existing,
        ),
        CompletionUnknownRow::AdministrativeReprobe { recovery_id } => boundary_unknown(
            event_id,
            recovery_id,
            InvalidationReason::AdministrativeReprobe,
            existing,
        ),
        CompletionUnknownRow::IdentityChangingHotplug { recovery_id } => boundary_unknown(
            event_id,
            recovery_id,
            InvalidationReason::IdentityChangingHotplug,
            existing,
        ),
    }
}

/// C.0 REC-6: decide an existing incident's fate for a winning lifecycle kind.
/// The function neither allocates IDs nor performs any side effect.
pub fn table_f(
    winner: RecoveryWinner,
    incident: RecoveryIncident,
    event_id: LifecycleEventId,
) -> TableFOutcome {
    if incident.state == RecoveryIncidentState::RecoveryFailed {
        return failed_incident_fate(winner, incident, event_id);
    }

    let reason = boundary_reason(winner.kind);
    if let Some(reason) = reason {
        let new_incident = winner.recovery_required.then_some(IncidentSeed::boundary());
        return TableFOutcome {
            fate: RecoveryFate {
                new_incident,
                invalidated_incident: Some(IncidentInvalidation {
                    recovery_id: incident.id,
                    reason,
                }),
                representative_disposition: representative_disposition(&incident, reason),
                ..RecoveryFate::default()
            },
            incident_state: winner
                .recovery_required
                .then_some(RecoveryIncidentState::Active),
            allocate_fresh_id: winner.recovery_required,
        };
    }

    match winner.kind {
        LifecycleKind::Shutdown | LifecycleKind::DeviceRemoved | LifecycleKind::VTRelease => {
            let reason = invalidation_reason(winner.kind)
                .expect("teardown kinds have an invalidation reason");
            TableFOutcome {
                fate: RecoveryFate {
                    invalidated_incident: Some(IncidentInvalidation {
                        recovery_id: incident.id,
                        reason,
                    }),
                    representative_disposition: representative_disposition(&incident, reason),
                    event_disposition: incident.representative().map(|representative| {
                        (event_id, Disposition::AbsorbedByEvent(representative))
                    }),
                    ..RecoveryFate::default()
                },
                incident_state: None,
                allocate_fresh_id: false,
            }
        }
        LifecycleKind::TopologyRebuild => TableFOutcome {
            fate: RecoveryFate {
                current_incident: Some(incident),
                transferred: true,
                ..RecoveryFate::default()
            },
            incident_state: Some(incident.state),
            allocate_fresh_id: false,
        },
        LifecycleKind::DPMS => {
            let mut next = incident;
            match winner.dpms_target {
                DpmsTarget::Off => {
                    next.pause();
                }
                DpmsTarget::On => {
                    next.resume();
                }
            }
            TableFOutcome {
                fate: RecoveryFate {
                    current_incident: Some(next),
                    ..RecoveryFate::default()
                },
                incident_state: Some(next.state),
                allocate_fresh_id: false,
            }
        }
        LifecycleKind::NormalRecovery => TableFOutcome {
            fate: RecoveryFate {
                current_incident: Some(incident),
                event_disposition: incident
                    .representative()
                    .map(|representative| (event_id, Disposition::AbsorbedByEvent(representative))),
                ..RecoveryFate::default()
            },
            incident_state: Some(incident.state),
            allocate_fresh_id: false,
        },
        // The four boundary kinds returned above.
        LifecycleKind::DeviceAddedOrReplaced
        | LifecycleKind::VTAcquire
        | LifecycleKind::AdministrativeReprobe
        | LifecycleKind::IdentityChangingHotplug => unreachable!("boundary handled above"),
    }
}

fn failed_incident_fate(
    winner: RecoveryWinner,
    incident: RecoveryIncident,
    event_id: LifecycleEventId,
) -> TableFOutcome {
    let boundary = boundary_reason(winner.kind).is_some();
    let new_incident = (boundary && winner.recovery_required).then_some(IncidentSeed::boundary());
    let later_loss = (winner.kind == LifecycleKind::NormalRecovery).then_some((
        event_id,
        Disposition::Invalidated(InvalidationReason::RecoveryFailed),
    ));
    let retain_failed = new_incident.is_none();
    TableFOutcome {
        fate: RecoveryFate {
            current_incident: retain_failed.then_some(incident),
            new_incident,
            event_disposition: later_loss,
            ..RecoveryFate::default()
        },
        incident_state: if boundary && winner.recovery_required {
            Some(RecoveryIncidentState::Active)
        } else {
            Some(RecoveryIncidentState::RecoveryFailed)
        },
        allocate_fresh_id: boundary && winner.recovery_required,
    }
}

fn normal_live(
    event_id: LifecycleEventId,
    existing: Option<RecoveryIncident>,
    terminalize_rebuild: bool,
) -> TableUOutcome {
    match existing {
        None => {
            let seed = IncidentSeed::completion_loss(event_id, RecoveryIncidentState::Active);
            TableUOutcome {
                logical: LogicalUnknownActions {
                    stop_admission: true,
                    withdraw_protocol_work: true,
                    release_seat: false,
                    revoke_readiness: true,
                    apply_dpms_target: false,
                    poison_device: true,
                    terminalize_rebuild,
                    event: EventFate::PendingRepresentative(event_id),
                },
                physical: if terminalize_rebuild {
                    PhysicalUnknownOutcome::QuarantineReapCloseThenAttemptOnceAndTerminalizeRebuild
                } else {
                    PhysicalUnknownOutcome::QuarantineReapCloseThenAttemptOnce
                },
                physical_barriers: PhysicalBarriers::UNKNOWN_COMMIT,
                recovery: RecoveryFate {
                    new_incident: Some(seed),
                    ..RecoveryFate::default()
                },
            }
        }
        Some(incident) if incident.state == RecoveryIncidentState::Attempting => {
            let resolution = incident.resolve(RecoveryResolution::Failed);
            TableUOutcome {
                logical: poison_event(
                    event_id,
                    event_fate_for(event_id, incident, InvalidationReason::RecoveryFailed),
                ),
                physical: PhysicalUnknownOutcome::ExistingAttemptFailedWithoutRetry,
                physical_barriers: PhysicalBarriers::UNKNOWN_COMMIT,
                recovery: RecoveryFate {
                    current_incident: resolution.incident,
                    representative_disposition: resolution.representative_disposition,
                    event_disposition: event_fate_disposition(event_id, incident),
                    ..RecoveryFate::default()
                },
            }
        }
        Some(incident) if incident.state == RecoveryIncidentState::RecoveryFailed => {
            TableUOutcome {
                logical: poison_event(
                    event_id,
                    terminal_event(event_id, InvalidationReason::RecoveryFailed),
                ),
                physical: PhysicalUnknownOutcome::ExistingFailureRemainsWithoutRetry,
                physical_barriers: PhysicalBarriers::UNKNOWN_COMMIT,
                recovery: RecoveryFate {
                    current_incident: Some(incident),
                    event_disposition: Some((
                        event_id,
                        Disposition::Invalidated(InvalidationReason::RecoveryFailed),
                    )),
                    ..RecoveryFate::default()
                },
            }
        }
        Some(incident) => TableUOutcome {
            logical: poison_event(
                event_id,
                event_fate_for(event_id, incident, InvalidationReason::RecoveryFailed),
            ),
            physical: PhysicalUnknownOutcome::QuarantineReapCloseThenAttemptOnce,
            physical_barriers: PhysicalBarriers::UNKNOWN_COMMIT,
            recovery: RecoveryFate {
                current_incident: Some(incident),
                event_disposition: event_fate_disposition(event_id, incident),
                ..RecoveryFate::default()
            },
        },
    }
}

fn teardown(
    event_id: LifecycleEventId,
    existing: Option<RecoveryIncident>,
    reason: InvalidationReason,
    mut logical: LogicalUnknownActions,
) -> TableUOutcome {
    let mut fate = RecoveryFate::default();
    if let Some(incident) = existing {
        if incident.state == RecoveryIncidentState::RecoveryFailed {
            fate.current_incident = Some(incident);
            logical.event = terminal_event(event_id, InvalidationReason::RecoveryFailed);
            fate.event_disposition = Some((
                event_id,
                Disposition::Invalidated(InvalidationReason::RecoveryFailed),
            ));
        } else {
            fate.invalidated_incident = Some(IncidentInvalidation {
                recovery_id: incident.id,
                reason,
            });
            fate.representative_disposition = representative_disposition(&incident, reason);
            logical.event = event_fate_for(event_id, incident, reason);
            fate.event_disposition = event_fate_disposition(event_id, incident);
        }
    } else {
        logical.event = terminal_event(event_id, reason);
        fate.event_disposition = Some((event_id, Disposition::Invalidated(reason)));
    }
    TableUOutcome {
        logical,
        physical: if reason == InvalidationReason::VTRelease {
            PhysicalUnknownOutcome::QuarantineAwaitLifecycleTeardown
        } else {
            PhysicalUnknownOutcome::QuarantineReapCloseWithoutAttempt
        },
        physical_barriers: PhysicalBarriers::UNKNOWN_COMMIT,
        recovery: fate,
    }
}

fn dpms_unknown(
    event_id: LifecycleEventId,
    target: DpmsTarget,
    existing: Option<RecoveryIncident>,
) -> TableUOutcome {
    let mut fate = match existing {
        None => RecoveryFate {
            new_incident: Some(IncidentSeed::completion_loss(
                event_id,
                match target {
                    DpmsTarget::On => RecoveryIncidentState::Active,
                    DpmsTarget::Off => RecoveryIncidentState::Paused,
                },
            )),
            ..RecoveryFate::default()
        },
        Some(incident) if incident.state == RecoveryIncidentState::Attempting => {
            let resolution = incident.resolve(RecoveryResolution::Failed);
            RecoveryFate {
                current_incident: resolution.incident,
                representative_disposition: resolution.representative_disposition,
                event_disposition: event_fate_disposition(event_id, incident),
                ..RecoveryFate::default()
            }
        }
        Some(incident) if incident.state == RecoveryIncidentState::RecoveryFailed => RecoveryFate {
            current_incident: Some(incident),
            event_disposition: Some((
                event_id,
                Disposition::Invalidated(InvalidationReason::RecoveryFailed),
            )),
            ..RecoveryFate::default()
        },
        Some(mut incident) => {
            match target {
                DpmsTarget::Off => {
                    incident.pause();
                }
                DpmsTarget::On => {
                    incident.resume();
                }
            }
            RecoveryFate {
                current_incident: Some(incident),
                event_disposition: event_fate_disposition(event_id, incident),
                ..RecoveryFate::default()
            }
        }
    };
    if let Some(incident) = existing
        && incident.state == RecoveryIncidentState::Attempting
    {
        fate.event_disposition = event_fate_disposition(event_id, incident);
    }
    let event = match (&fate.new_incident, fate.event_disposition) {
        (Some(seed), _) => match seed.origin {
            IncidentOrigin::CompletionLoss { representative } => {
                EventFate::PendingRepresentative(representative)
            }
            IncidentOrigin::Boundary => unreachable!("DPMS cannot create a boundary incident"),
        },
        (_, Some((event_id, disposition))) => EventFate::Terminal {
            event_id,
            disposition,
        },
        _ => terminal_event(event_id, InvalidationReason::RecoveryFailed),
    };
    let attempt_after_reap = fate
        .new_incident
        .is_some_and(|seed| seed.initial_state == RecoveryIncidentState::Active)
        || fate.current_incident.is_some_and(|incident| {
            matches!(
                incident.state,
                RecoveryIncidentState::Active | RecoveryIncidentState::Attempting
            )
        });
    let paused_until_dpms_on = target == DpmsTarget::Off
        && fate
            .current_incident
            .is_some_and(|incident| incident.state == RecoveryIncidentState::Paused)
        || fate
            .new_incident
            .is_some_and(|seed| seed.initial_state == RecoveryIncidentState::Paused);
    TableUOutcome {
        logical: LogicalUnknownActions {
            stop_admission: true,
            withdraw_protocol_work: false,
            release_seat: false,
            revoke_readiness: true,
            apply_dpms_target: true,
            poison_device: true,
            terminalize_rebuild: false,
            event,
        },
        physical: PhysicalUnknownOutcome::QuarantineDpmsWithoutKmsMutation {
            attempt_after_reap,
            paused_until_dpms_on,
        },
        physical_barriers: PhysicalBarriers::UNKNOWN_COMMIT,
        recovery: fate,
    }
}

fn boundary_unknown(
    event_id: LifecycleEventId,
    recovery_id: RecoveryId,
    reason: InvalidationReason,
    existing: Option<RecoveryIncident>,
) -> TableUOutcome {
    let mut fate = RecoveryFate {
        current_incident: Some(RecoveryIncident::failed_boundary(recovery_id)),
        ..RecoveryFate::default()
    };
    let event = if let Some(incident) = existing {
        if incident.state == RecoveryIncidentState::RecoveryFailed {
            terminal_event(event_id, InvalidationReason::RecoveryFailed)
        } else {
            fate.invalidated_incident = Some(IncidentInvalidation {
                recovery_id: incident.id,
                reason,
            });
            fate.representative_disposition = representative_disposition(&incident, reason);
            event_fate_for(event_id, incident, InvalidationReason::RecoveryFailed)
        }
    } else {
        terminal_event(event_id, InvalidationReason::RecoveryFailed)
    };
    if let EventFate::Terminal {
        event_id,
        disposition,
    } = event
    {
        fate.event_disposition = Some((event_id, disposition));
    } else if let EventFate::PendingRepresentative(representative) = event {
        fate.event_disposition = Some((event_id, Disposition::AbsorbedByEvent(representative)));
    }
    TableUOutcome {
        logical: LogicalUnknownActions {
            stop_admission: true,
            withdraw_protocol_work: false,
            release_seat: false,
            revoke_readiness: true,
            apply_dpms_target: false,
            poison_device: true,
            terminalize_rebuild: false,
            event,
        },
        physical: PhysicalUnknownOutcome::QuarantineBoundaryAttemptFailed { recovery_id },
        physical_barriers: PhysicalBarriers::UNKNOWN_COMMIT,
        recovery: fate,
    }
}

fn poison_event(event_id: LifecycleEventId, event: EventFate) -> LogicalUnknownActions {
    let _ = event_id;
    LogicalUnknownActions {
        stop_admission: true,
        withdraw_protocol_work: true,
        release_seat: false,
        revoke_readiness: true,
        apply_dpms_target: false,
        poison_device: true,
        terminalize_rebuild: false,
        event,
    }
}

fn absorbed_fate(event_id: LifecycleEventId, incident: RecoveryIncident) -> RecoveryFate {
    RecoveryFate {
        current_incident: Some(incident),
        event_disposition: event_fate_disposition(event_id, incident),
        ..RecoveryFate::default()
    }
}

fn terminal_event(event_id: LifecycleEventId, reason: InvalidationReason) -> EventFate {
    EventFate::Terminal {
        event_id,
        disposition: Disposition::Invalidated(reason),
    }
}

fn event_fate_for(
    event_id: LifecycleEventId,
    incident: RecoveryIncident,
    fallback: InvalidationReason,
) -> EventFate {
    match incident.representative() {
        Some(representative) => EventFate::Terminal {
            event_id,
            disposition: Disposition::AbsorbedByEvent(representative),
        },
        None => terminal_event(event_id, fallback),
    }
}

fn event_fate_disposition(
    event_id: LifecycleEventId,
    incident: RecoveryIncident,
) -> Option<(LifecycleEventId, Disposition)> {
    incident
        .representative()
        .map(|representative| (event_id, Disposition::AbsorbedByEvent(representative)))
}

fn representative_disposition(
    incident: &RecoveryIncident,
    reason: InvalidationReason,
) -> Option<(LifecycleEventId, Disposition)> {
    incident
        .representative()
        .map(|event_id| (event_id, Disposition::Invalidated(reason)))
}

fn invalidation_reason(kind: LifecycleKind) -> Option<InvalidationReason> {
    match kind {
        LifecycleKind::Shutdown => Some(InvalidationReason::Shutdown),
        LifecycleKind::DeviceRemoved => Some(InvalidationReason::DeviceRemoved),
        LifecycleKind::VTRelease => Some(InvalidationReason::VTRelease),
        _ => None,
    }
}

fn boundary_reason(kind: LifecycleKind) -> Option<InvalidationReason> {
    match kind {
        LifecycleKind::DeviceAddedOrReplaced => Some(InvalidationReason::DeviceAddedOrReplaced),
        LifecycleKind::VTAcquire => Some(InvalidationReason::VTAcquire),
        LifecycleKind::AdministrativeReprobe => Some(InvalidationReason::AdministrativeReprobe),
        LifecycleKind::IdentityChangingHotplug => Some(InvalidationReason::IdentityChangingHotplug),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CompletionUnknownRow, CompletionUnknownRowKind, DpmsTarget, EventFate, IncidentOrigin,
        IncidentSeed, LogicalUnknownActions, PhysicalBarriers, PhysicalUnknownOutcome,
        RecoveryAttemptBudget, RecoveryAttemptTrigger, RecoveryIdAllocator, RecoveryIncident,
        RecoveryIncidentState, RecoveryResolution, RecoveryWinner, table_f, table_u,
    };
    use crate::kms::owner::lifecycle::{
        DesiredIntent, Disposition, InvalidationReason, LifecycleDesired, LifecycleEventId,
        LifecycleKind, LifecycleTransitionId, Prerequisite,
    };

    fn event(raw: u64) -> LifecycleEventId {
        LifecycleEventId::from_raw(raw)
    }

    fn incident(state: RecoveryIncidentState, representative: Option<u64>) -> RecoveryIncident {
        let origin = representative.map_or(IncidentOrigin::Boundary, |raw| {
            IncidentOrigin::CompletionLoss {
                representative: event(raw),
            }
        });
        let seed = IncidentSeed {
            origin,
            initial_state: match state {
                RecoveryIncidentState::Active | RecoveryIncidentState::Paused => {
                    RecoveryIncidentState::Active
                }
                RecoveryIncidentState::Attempting | RecoveryIncidentState::RecoveryFailed => {
                    RecoveryIncidentState::Active
                }
            },
        };
        let mut result =
            RecoveryIncident::from_seed(RecoveryIdAllocator::new().allocate().unwrap(), seed);
        match state {
            RecoveryIncidentState::Active => {}
            RecoveryIncidentState::Paused => assert!(result.pause()),
            RecoveryIncidentState::Attempting => assert!(result.begin_attempt()),
            RecoveryIncidentState::RecoveryFailed => {
                assert!(result.begin_attempt());
                result = result.resolve(RecoveryResolution::Failed).incident.unwrap();
            }
        }
        result
    }

    fn row(kind: CompletionUnknownRowKind) -> CompletionUnknownRow {
        let id = crate::kms::owner::lifecycle::RecoveryId::from_raw(
            100 + CompletionUnknownRowKind::ALL
                .iter()
                .position(|candidate| *candidate == kind)
                .unwrap() as u64,
        );
        match kind {
            CompletionUnknownRowKind::NormalLive => CompletionUnknownRow::NormalLive,
            CompletionUnknownRowKind::VTRelease => CompletionUnknownRow::VTRelease,
            CompletionUnknownRowKind::DeviceRemoved => CompletionUnknownRow::DeviceRemoved,
            CompletionUnknownRowKind::Shutdown => CompletionUnknownRow::Shutdown,
            CompletionUnknownRowKind::DeviceAddedOrReplaced => {
                CompletionUnknownRow::DeviceAddedOrReplaced { recovery_id: id }
            }
            CompletionUnknownRowKind::VTAcquire => {
                CompletionUnknownRow::VTAcquire { recovery_id: id }
            }
            CompletionUnknownRowKind::AdministrativeReprobe => {
                CompletionUnknownRow::AdministrativeReprobe { recovery_id: id }
            }
            CompletionUnknownRowKind::IdentityChangingHotplug => {
                CompletionUnknownRow::IdentityChangingHotplug { recovery_id: id }
            }
            CompletionUnknownRowKind::TopologyRebuild => CompletionUnknownRow::TopologyRebuild,
            CompletionUnknownRowKind::DPMS => CompletionUnknownRow::DPMS {
                target: DpmsTarget::On,
            },
        }
    }

    #[test]
    fn c0_3a_unknown_table_by_row() {
        for kind in CompletionUnknownRowKind::ALL {
            let current = incident(RecoveryIncidentState::Active, Some(40));
            let no_incident = table_u(row(kind), event(80), None);
            let with_incident = table_u(row(kind), event(81), Some(current));
            let expected_barriers = PhysicalBarriers {
                quarantine_both_possible_states: true,
                stop_alias_creation: true,
                request_executor_and_helper_termination: true,
                reap_every_lease: true,
                close_complete_fd_family_after_reap: true,
            };
            assert_eq!(no_incident.physical_barriers, expected_barriers, "{kind:?}");
            assert_eq!(
                with_incident.physical_barriers, expected_barriers,
                "{kind:?}"
            );

            match kind {
                CompletionUnknownRowKind::NormalLive => {
                    let seed = no_incident.recovery.new_incident.unwrap();
                    assert_eq!(
                        seed,
                        IncidentSeed::completion_loss(event(80), RecoveryIncidentState::Active)
                    );
                    assert_eq!(
                        no_incident.logical.event,
                        EventFate::PendingRepresentative(event(80))
                    );
                    assert_eq!(
                        no_incident.logical,
                        LogicalUnknownActions {
                            stop_admission: true,
                            withdraw_protocol_work: true,
                            release_seat: false,
                            revoke_readiness: true,
                            apply_dpms_target: false,
                            poison_device: true,
                            terminalize_rebuild: false,
                            event: EventFate::PendingRepresentative(event(80)),
                        }
                    );
                    assert!(no_incident.logical.stop_admission);
                    assert!(no_incident.logical.withdraw_protocol_work);
                    assert!(no_incident.logical.poison_device);
                    assert_eq!(
                        no_incident.physical,
                        PhysicalUnknownOutcome::QuarantineReapCloseThenAttemptOnce
                    );
                    assert_eq!(with_incident.recovery.current_incident, Some(current));
                    assert_eq!(with_incident.recovery.new_incident, None);
                    assert_eq!(
                        with_incident.logical.event,
                        EventFate::Terminal {
                            event_id: event(81),
                            disposition: Disposition::AbsorbedByEvent(event(40)),
                        }
                    );
                    assert_eq!(
                        with_incident.physical,
                        PhysicalUnknownOutcome::QuarantineReapCloseThenAttemptOnce
                    );
                    assert_eq!(
                        with_incident.recovery.event_disposition,
                        Some((event(81), Disposition::AbsorbedByEvent(event(40))))
                    );
                }
                CompletionUnknownRowKind::VTRelease
                | CompletionUnknownRowKind::DeviceRemoved
                | CompletionUnknownRowKind::Shutdown => {
                    let reason = match kind {
                        CompletionUnknownRowKind::VTRelease => InvalidationReason::VTRelease,
                        CompletionUnknownRowKind::DeviceRemoved => {
                            InvalidationReason::DeviceRemoved
                        }
                        CompletionUnknownRowKind::Shutdown => InvalidationReason::Shutdown,
                        _ => unreachable!(),
                    };
                    let (withdraw_protocol_work, release_seat) = match kind {
                        CompletionUnknownRowKind::VTRelease => (false, true),
                        CompletionUnknownRowKind::DeviceRemoved => (true, false),
                        CompletionUnknownRowKind::Shutdown => (true, true),
                        _ => unreachable!(),
                    };
                    let expected_actions = |event| LogicalUnknownActions {
                        stop_admission: true,
                        withdraw_protocol_work,
                        release_seat,
                        revoke_readiness: true,
                        apply_dpms_target: false,
                        poison_device: false,
                        terminalize_rebuild: false,
                        event,
                    };
                    assert_eq!(
                        no_incident.logical.event,
                        EventFate::Terminal {
                            event_id: event(80),
                            disposition: Disposition::Invalidated(reason),
                        }
                    );
                    assert_eq!(
                        no_incident.logical,
                        expected_actions(EventFate::Terminal {
                            event_id: event(80),
                            disposition: Disposition::Invalidated(reason),
                        })
                    );
                    assert_eq!(no_incident.recovery.current_incident, None);
                    assert_eq!(no_incident.recovery.new_incident, None);
                    assert_eq!(
                        no_incident.recovery.event_disposition,
                        Some((event(80), Disposition::Invalidated(reason)))
                    );
                    assert_eq!(
                        no_incident.physical,
                        if kind == CompletionUnknownRowKind::VTRelease {
                            PhysicalUnknownOutcome::QuarantineAwaitLifecycleTeardown
                        } else {
                            PhysicalUnknownOutcome::QuarantineReapCloseWithoutAttempt
                        }
                    );
                    assert!(no_incident.logical.stop_admission, "{kind:?}");
                    if kind == CompletionUnknownRowKind::VTRelease {
                        assert!(no_incident.logical.release_seat);
                    }
                    if kind == CompletionUnknownRowKind::DeviceRemoved {
                        assert!(no_incident.logical.withdraw_protocol_work);
                    }
                    assert_eq!(
                        with_incident.recovery.invalidated_incident.unwrap().reason,
                        reason
                    );
                    assert_eq!(
                        with_incident.recovery.representative_disposition,
                        Some((event(40), Disposition::Invalidated(reason)))
                    );
                    assert_eq!(
                        with_incident
                            .recovery
                            .invalidated_incident
                            .unwrap()
                            .recovery_id,
                        current.id()
                    );
                    assert_eq!(
                        with_incident.logical.event,
                        EventFate::Terminal {
                            event_id: event(81),
                            disposition: Disposition::AbsorbedByEvent(event(40)),
                        }
                    );
                    assert_eq!(
                        with_incident.logical,
                        expected_actions(EventFate::Terminal {
                            event_id: event(81),
                            disposition: Disposition::AbsorbedByEvent(event(40)),
                        })
                    );
                }
                CompletionUnknownRowKind::TopologyRebuild => {
                    assert_eq!(
                        no_incident.recovery.new_incident,
                        Some(IncidentSeed::completion_loss(
                            event(80),
                            RecoveryIncidentState::Active
                        ))
                    );
                    assert!(no_incident.logical.terminalize_rebuild);
                    assert_eq!(
                        no_incident.physical,
                        PhysicalUnknownOutcome::QuarantineReapCloseThenAttemptOnceAndTerminalizeRebuild
                    );
                    assert_eq!(with_incident.recovery.current_incident, Some(current));
                    assert!(with_incident.recovery.transferred);
                    assert_eq!(with_incident.recovery.new_incident, None);
                    assert_eq!(
                        with_incident.physical,
                        PhysicalUnknownOutcome::TransferQuarantineToRebuildThenQualifiedInstall
                    );
                    assert_eq!(
                        with_incident.recovery.event_disposition,
                        Some((event(81), Disposition::AbsorbedByEvent(event(40))))
                    );
                    assert_eq!(
                        with_incident.logical,
                        LogicalUnknownActions {
                            stop_admission: true,
                            withdraw_protocol_work: false,
                            release_seat: false,
                            revoke_readiness: true,
                            apply_dpms_target: false,
                            poison_device: true,
                            terminalize_rebuild: false,
                            event: EventFate::Terminal {
                                event_id: event(81),
                                disposition: Disposition::AbsorbedByEvent(event(40)),
                            },
                        }
                    );
                    let attempting = incident(RecoveryIncidentState::Attempting, Some(40));
                    let failed_attempt = table_u(
                        CompletionUnknownRow::TopologyRebuild,
                        event(82),
                        Some(attempting),
                    );
                    assert_eq!(
                        failed_attempt.physical,
                        PhysicalUnknownOutcome::ExistingAttemptFailedWithoutRetry
                    );
                    assert!(!failed_attempt.recovery.transferred);
                    assert_eq!(
                        failed_attempt.recovery.current_incident.unwrap().state(),
                        RecoveryIncidentState::RecoveryFailed
                    );
                    assert_eq!(
                        failed_attempt.recovery.representative_disposition,
                        Some((
                            event(40),
                            Disposition::Invalidated(InvalidationReason::RecoveryFailed)
                        ))
                    );
                }
                CompletionUnknownRowKind::DPMS => {
                    assert_eq!(
                        no_incident.recovery.new_incident,
                        Some(IncidentSeed::completion_loss(
                            event(80),
                            RecoveryIncidentState::Active
                        ))
                    );
                    assert_eq!(
                        no_incident.physical,
                        PhysicalUnknownOutcome::QuarantineDpmsWithoutKmsMutation {
                            attempt_after_reap: true,
                            paused_until_dpms_on: false,
                        }
                    );
                    assert_eq!(
                        no_incident.logical.event,
                        EventFate::PendingRepresentative(event(80))
                    );
                    assert_eq!(with_incident.recovery.current_incident, Some(current));
                    assert_eq!(with_incident.recovery.new_incident, None);
                    assert_eq!(
                        with_incident.physical,
                        PhysicalUnknownOutcome::QuarantineDpmsWithoutKmsMutation {
                            attempt_after_reap: true,
                            paused_until_dpms_on: false,
                        }
                    );
                    assert!(!with_incident.logical.withdraw_protocol_work);
                    assert!(with_incident.logical.apply_dpms_target);
                    assert_eq!(
                        with_incident.logical.event,
                        EventFate::Terminal {
                            event_id: event(81),
                            disposition: Disposition::AbsorbedByEvent(event(40)),
                        }
                    );
                    assert_eq!(
                        with_incident.recovery.event_disposition,
                        Some((event(81), Disposition::AbsorbedByEvent(event(40))))
                    );
                }
                CompletionUnknownRowKind::DeviceAddedOrReplaced
                | CompletionUnknownRowKind::VTAcquire
                | CompletionUnknownRowKind::AdministrativeReprobe
                | CompletionUnknownRowKind::IdentityChangingHotplug => {
                    let expected_reason = match kind {
                        CompletionUnknownRowKind::DeviceAddedOrReplaced => {
                            InvalidationReason::DeviceAddedOrReplaced
                        }
                        CompletionUnknownRowKind::VTAcquire => InvalidationReason::VTAcquire,
                        CompletionUnknownRowKind::AdministrativeReprobe => {
                            InvalidationReason::AdministrativeReprobe
                        }
                        CompletionUnknownRowKind::IdentityChangingHotplug => {
                            InvalidationReason::IdentityChangingHotplug
                        }
                        _ => unreachable!(),
                    };
                    let fresh_id = match row(kind) {
                        CompletionUnknownRow::DeviceAddedOrReplaced { recovery_id }
                        | CompletionUnknownRow::VTAcquire { recovery_id }
                        | CompletionUnknownRow::AdministrativeReprobe { recovery_id }
                        | CompletionUnknownRow::IdentityChangingHotplug { recovery_id } => {
                            recovery_id
                        }
                        _ => unreachable!(),
                    };
                    assert_eq!(
                        no_incident.physical,
                        PhysicalUnknownOutcome::QuarantineBoundaryAttemptFailed {
                            recovery_id: fresh_id
                        }
                    );
                    assert_eq!(
                        no_incident.logical.event,
                        EventFate::Terminal {
                            event_id: event(80),
                            disposition: Disposition::Invalidated(
                                InvalidationReason::RecoveryFailed
                            ),
                        }
                    );
                    assert_eq!(
                        no_incident.recovery.current_incident.unwrap().id(),
                        fresh_id
                    );
                    assert_eq!(
                        no_incident.recovery.current_incident.unwrap().state(),
                        RecoveryIncidentState::RecoveryFailed
                    );
                    assert_eq!(
                        no_incident.recovery.event_disposition,
                        Some((
                            event(80),
                            Disposition::Invalidated(InvalidationReason::RecoveryFailed)
                        ))
                    );
                    assert_eq!(
                        with_incident.recovery.invalidated_incident.unwrap().reason,
                        expected_reason
                    );
                    assert_eq!(
                        with_incident.recovery.event_disposition,
                        Some((event(81), Disposition::AbsorbedByEvent(event(40))))
                    );
                    assert_eq!(
                        with_incident.recovery.current_incident.unwrap().id(),
                        fresh_id
                    );
                    assert_eq!(
                        with_incident.logical.event,
                        EventFate::Terminal {
                            event_id: event(81),
                            disposition: Disposition::AbsorbedByEvent(event(40)),
                        }
                    );
                    assert_eq!(
                        with_incident.recovery.event_disposition,
                        Some((event(81), Disposition::AbsorbedByEvent(event(40))))
                    );
                }
            }
        }

        let dpms_off = table_u(
            CompletionUnknownRow::DPMS {
                target: DpmsTarget::Off,
            },
            event(90),
            None,
        );
        assert_eq!(
            dpms_off.recovery.new_incident.unwrap().initial_state,
            RecoveryIncidentState::Paused
        );
        assert_eq!(
            dpms_off.physical,
            PhysicalUnknownOutcome::QuarantineDpmsWithoutKmsMutation {
                attempt_after_reap: false,
                paused_until_dpms_on: true,
            }
        );
    }

    #[test]
    fn c0_3a_recovery_matrix_is_total() {
        let loss_incident = incident(RecoveryIncidentState::Active, Some(10));
        let failed_incident = incident(RecoveryIncidentState::RecoveryFailed, Some(10));
        for kind in LifecycleKind::ALL {
            let winner = RecoveryWinner::new(kind).requiring_recovery();
            let outcome = table_f(winner, loss_incident, event(20));
            match kind {
                LifecycleKind::Shutdown
                | LifecycleKind::DeviceRemoved
                | LifecycleKind::VTRelease => {
                    let reason = match kind {
                        LifecycleKind::Shutdown => InvalidationReason::Shutdown,
                        LifecycleKind::DeviceRemoved => InvalidationReason::DeviceRemoved,
                        LifecycleKind::VTRelease => InvalidationReason::VTRelease,
                        _ => unreachable!(),
                    };
                    assert_eq!(
                        outcome.fate.invalidated_incident.unwrap().reason,
                        reason,
                        "{kind:?}"
                    );
                    assert_eq!(outcome.fate.current_incident, None, "{kind:?}");
                    assert_eq!(outcome.fate.new_incident, None, "{kind:?}");
                    assert_eq!(
                        outcome.fate.representative_disposition,
                        Some((event(10), Disposition::Invalidated(reason))),
                        "{kind:?}"
                    );
                    assert!(!outcome.allocate_fresh_id, "{kind:?}");
                }
                LifecycleKind::DeviceAddedOrReplaced
                | LifecycleKind::VTAcquire
                | LifecycleKind::AdministrativeReprobe
                | LifecycleKind::IdentityChangingHotplug => {
                    assert_eq!(
                        outcome.fate.invalidated_incident.unwrap().reason,
                        match kind {
                            LifecycleKind::DeviceAddedOrReplaced => {
                                InvalidationReason::DeviceAddedOrReplaced
                            }
                            LifecycleKind::VTAcquire => InvalidationReason::VTAcquire,
                            LifecycleKind::AdministrativeReprobe => {
                                InvalidationReason::AdministrativeReprobe
                            }
                            LifecycleKind::IdentityChangingHotplug => {
                                InvalidationReason::IdentityChangingHotplug
                            }
                            _ => unreachable!(),
                        },
                        "{kind:?}"
                    );
                    assert_eq!(outcome.fate.new_incident.unwrap(), IncidentSeed::boundary());
                    assert_eq!(
                        outcome.fate.representative_disposition,
                        Some((
                            event(10),
                            Disposition::Invalidated(
                                outcome.fate.invalidated_incident.unwrap().reason
                            )
                        ))
                    );
                    assert_eq!(outcome.fate.current_incident, None);
                    assert!(outcome.allocate_fresh_id, "{kind:?}");
                    let mut allocator = RecoveryIdAllocator::new();
                    let boundary_incident = allocator
                        .allocate_incident(outcome.fate.new_incident.unwrap())
                        .unwrap();
                    assert_eq!(
                        boundary_incident.id(),
                        crate::kms::owner::lifecycle::RecoveryId::from_raw(1)
                    );
                    assert_eq!(boundary_incident.representative(), None);
                    assert_eq!(boundary_incident.state(), RecoveryIncidentState::Active);
                    assert_eq!(
                        allocator.allocate(),
                        Some(crate::kms::owner::lifecycle::RecoveryId::from_raw(2))
                    );
                    let no_recovery = table_f(RecoveryWinner::new(kind), loss_incident, event(20));
                    assert_eq!(no_recovery.fate.new_incident, None, "{kind:?}");
                    assert!(!no_recovery.allocate_fresh_id, "{kind:?}");
                }
                LifecycleKind::TopologyRebuild => {
                    assert_eq!(outcome.fate.current_incident, Some(loss_incident));
                    assert!(outcome.fate.transferred);
                    assert_eq!(outcome.fate.invalidated_incident, None);
                    assert_eq!(outcome.fate.new_incident, None);
                    assert!(!outcome.allocate_fresh_id);
                }
                LifecycleKind::DPMS => {
                    assert_eq!(outcome.fate.current_incident, Some(loss_incident));
                    assert!(!outcome.fate.transferred);
                    assert_eq!(outcome.fate.invalidated_incident, None);
                    assert_eq!(outcome.fate.new_incident, None);
                    assert!(!outcome.allocate_fresh_id);
                }
                LifecycleKind::NormalRecovery => {
                    assert_eq!(outcome.fate.current_incident, Some(loss_incident));
                    assert_eq!(
                        outcome.fate.event_disposition,
                        Some((event(20), Disposition::AbsorbedByEvent(event(10))))
                    );
                    assert_eq!(outcome.fate.new_incident, None);
                    assert!(!outcome.allocate_fresh_id);
                }
            }

            let failed = table_f(winner, failed_incident, event(21));
            if matches!(
                kind,
                LifecycleKind::DeviceAddedOrReplaced
                    | LifecycleKind::VTAcquire
                    | LifecycleKind::AdministrativeReprobe
                    | LifecycleKind::IdentityChangingHotplug
            ) {
                assert!(failed.allocate_fresh_id, "{kind:?}");
                assert_eq!(failed.fate.new_incident, Some(IncidentSeed::boundary()));
                assert_eq!(failed.fate.current_incident, None);
                assert_eq!(failed.incident_state, Some(RecoveryIncidentState::Active));
            } else {
                assert!(!failed.allocate_fresh_id, "{kind:?}");
                assert_eq!(failed.fate.new_incident, None);
                assert_eq!(failed.fate.current_incident, Some(failed_incident));
                assert_eq!(
                    failed.incident_state,
                    Some(RecoveryIncidentState::RecoveryFailed)
                );
            }
            if kind == LifecycleKind::NormalRecovery {
                assert_eq!(
                    failed.fate.event_disposition,
                    Some((
                        event(21),
                        Disposition::Invalidated(InvalidationReason::RecoveryFailed)
                    ))
                );
            }
        }

        for state in RecoveryIncidentState::ALL {
            let candidate = incident(state, Some(11));
            for kind in LifecycleKind::ALL {
                let outcome = table_f(RecoveryWinner::new(kind), candidate, event(22));
                if kind == LifecycleKind::TopologyRebuild
                    && state != RecoveryIncidentState::RecoveryFailed
                {
                    assert_eq!(outcome.fate.current_incident, Some(candidate));
                    assert!(outcome.fate.transferred);
                }
                if state == RecoveryIncidentState::RecoveryFailed {
                    assert!(!outcome.allocate_fresh_id);
                    assert_eq!(outcome.fate.current_incident, Some(candidate));
                }
                if kind == LifecycleKind::DPMS {
                    let target = if state == RecoveryIncidentState::Paused {
                        DpmsTarget::On
                    } else {
                        DpmsTarget::Off
                    };
                    let outcome = table_f(
                        RecoveryWinner::new(kind).with_dpms_target(target),
                        candidate,
                        event(23),
                    );
                    let current = outcome.fate.current_incident.unwrap();
                    assert_eq!(current.id(), candidate.id());
                    assert_eq!(current.budget(), candidate.budget());
                    assert!(!outcome.allocate_fresh_id);
                    let expected = match (state, target) {
                        (RecoveryIncidentState::Active, DpmsTarget::Off) => {
                            RecoveryIncidentState::Paused
                        }
                        (RecoveryIncidentState::Paused, DpmsTarget::On) => {
                            RecoveryIncidentState::Active
                        }
                        (RecoveryIncidentState::Attempting, DpmsTarget::Off) => {
                            RecoveryIncidentState::Paused
                        }
                        (RecoveryIncidentState::RecoveryFailed, _) => {
                            RecoveryIncidentState::RecoveryFailed
                        }
                        (state, _) => state,
                    };
                    assert_eq!(current.state(), expected);
                }
            }
        }
    }

    #[test]
    fn c0_3a_no_double_fate() {
        for kind in CompletionUnknownRowKind::ALL {
            for existing in [
                None,
                Some(incident(RecoveryIncidentState::Active, Some(30))),
            ] {
                let outcome = table_u(row(kind), event(31), existing);
                let fate = outcome.recovery;
                assert!(
                    !fate.transferred || fate.invalidated_incident.is_none(),
                    "{kind:?}"
                );
                if let (Some(invalidated), Some(current)) =
                    (fate.invalidated_incident, fate.current_incident)
                {
                    assert_ne!(invalidated.recovery_id, current.id(), "{kind:?}");
                }
                assert!(fate.new_incident.is_none() || fate.current_incident.is_none());
            }
        }
        for kind in LifecycleKind::ALL {
            for current in [
                incident(RecoveryIncidentState::Active, Some(50)),
                incident(RecoveryIncidentState::RecoveryFailed, Some(50)),
            ] {
                let outcome = table_f(
                    RecoveryWinner::new(kind).requiring_recovery(),
                    current,
                    event(51),
                );
                let fate = outcome.fate;
                assert!(
                    !fate.transferred || fate.invalidated_incident.is_none(),
                    "{kind:?}"
                );
                if let (Some(invalidated), Some(retained)) =
                    (fate.invalidated_incident, fate.current_incident)
                {
                    assert_ne!(invalidated.recovery_id, retained.id(), "{kind:?}");
                }
                assert!(fate.new_incident.is_none() || fate.current_incident.is_none());
                assert_eq!(outcome.allocate_fresh_id, fate.new_incident.is_some());
            }
        }
    }

    #[test]
    fn c0_3a_first_loss_creates_one_incident() {
        let first = table_u(CompletionUnknownRow::NormalLive, event(1), None);
        assert_eq!(
            first.logical.event,
            EventFate::PendingRepresentative(event(1))
        );
        let mut allocator = RecoveryIdAllocator::new();
        let created = allocator
            .allocate_incident(first.recovery.new_incident.unwrap())
            .unwrap();
        assert_eq!(
            created.id(),
            crate::kms::owner::lifecycle::RecoveryId::from_raw(1)
        );
        assert_eq!(created.representative(), Some(event(1)));
        assert_eq!(created.state(), RecoveryIncidentState::Active);
        assert_eq!(created.budget(), RecoveryAttemptBudget::Available);

        let second_loss = table_u(CompletionUnknownRow::NormalLive, event(2), Some(created));
        assert_eq!(second_loss.recovery.new_incident, None);
        assert_eq!(second_loss.recovery.current_incident, Some(created));
        assert_eq!(
            second_loss.logical.event,
            EventFate::Terminal {
                event_id: event(2),
                disposition: Disposition::AbsorbedByEvent(event(1)),
            }
        );
        for duplicate in [event(3), event(4), event(5)] {
            let outcome = table_f(
                RecoveryWinner::new(LifecycleKind::NormalRecovery),
                created,
                duplicate,
            );
            assert_eq!(outcome.fate.current_incident, Some(created));
            assert_eq!(outcome.fate.new_incident, None);
            assert_eq!(
                outcome.fate.event_disposition,
                Some((duplicate, Disposition::AbsorbedByEvent(event(1))))
            );
        }

        let mut attempt = created;
        assert!(attempt.request_attempt(RecoveryAttemptTrigger::AfterReap));
        let success = attempt.resolve(RecoveryResolution::Qualified(
            LifecycleTransitionId::from_raw(9),
        ));
        assert_eq!(success.incident, None);
        assert_eq!(
            success.representative_disposition,
            Some((
                event(1),
                Disposition::Applied(LifecycleTransitionId::from_raw(9))
            ))
        );

        let mut failing_allocator = RecoveryIdAllocator::new();
        let mut failing = failing_allocator
            .allocate_incident(IncidentSeed::completion_loss(
                event(6),
                RecoveryIncidentState::Active,
            ))
            .unwrap();
        assert!(failing.request_attempt(RecoveryAttemptTrigger::AfterReap));
        let failed = failing.resolve(RecoveryResolution::Failed);
        assert_eq!(
            failed.representative_disposition,
            Some((
                event(6),
                Disposition::Invalidated(InvalidationReason::RecoveryFailed)
            ))
        );
        assert_eq!(
            failed.incident.unwrap().state(),
            RecoveryIncidentState::RecoveryFailed
        );

        let mut exhausted = RecoveryIdAllocator::with_next(
            crate::kms::owner::lifecycle::RecoveryId::from_raw(u64::MAX),
        );
        assert!(exhausted.allocate().is_some());
        assert_eq!(exhausted.allocate(), None);
    }

    #[test]
    fn c0_3a_unknown_during_teardown_follows_the_active_row() {
        for (row, reason, releases_seat, withdraws) in [
            (
                CompletionUnknownRow::VTRelease,
                InvalidationReason::VTRelease,
                true,
                false,
            ),
            (
                CompletionUnknownRow::DeviceRemoved,
                InvalidationReason::DeviceRemoved,
                false,
                true,
            ),
            (
                CompletionUnknownRow::Shutdown,
                InvalidationReason::Shutdown,
                true,
                true,
            ),
        ] {
            let result = table_u(row, event(60), None);
            assert_eq!(
                result.logical.event,
                EventFate::Terminal {
                    event_id: event(60),
                    disposition: Disposition::Invalidated(reason),
                }
            );
            assert_eq!(result.recovery.new_incident, None);
            assert_eq!(result.recovery.current_incident, None);
            assert!(result.logical.stop_admission);
            assert_eq!(result.logical.release_seat, releases_seat);
            assert_eq!(result.logical.withdraw_protocol_work, withdraws);
        }
    }

    #[test]
    fn c0_3a_first_loss_during_dpms_on_is_not_paused() {
        for (target, expected_state, attempt_after_reap) in [
            (DpmsTarget::On, RecoveryIncidentState::Active, true),
            (DpmsTarget::Off, RecoveryIncidentState::Paused, false),
        ] {
            let result = table_u(CompletionUnknownRow::DPMS { target }, event(70), None);
            assert_eq!(
                result.recovery.new_incident.unwrap().initial_state,
                expected_state
            );
            assert_eq!(
                result.physical,
                PhysicalUnknownOutcome::QuarantineDpmsWithoutKmsMutation {
                    attempt_after_reap,
                    paused_until_dpms_on: target == DpmsTarget::Off,
                }
            );
            assert!(result.logical.apply_dpms_target);
            assert!(result.logical.poison_device);
        }
    }

    #[test]
    fn c0_3a_first_loss_during_rebuild_is_normal_live() {
        let outcome = table_u(CompletionUnknownRow::TopologyRebuild, event(71), None);
        assert_eq!(
            outcome.recovery.new_incident,
            Some(IncidentSeed::completion_loss(
                event(71),
                RecoveryIncidentState::Active
            ))
        );
        assert_eq!(
            outcome.logical.event,
            EventFate::PendingRepresentative(event(71))
        );
        assert!(outcome.logical.terminalize_rebuild);
        assert_eq!(
            outcome.physical,
            PhysicalUnknownOutcome::QuarantineReapCloseThenAttemptOnceAndTerminalizeRebuild
        );
    }

    #[test]
    fn c0_3a_dpms_on_resumes_the_paused_incident() {
        let mut allocator = RecoveryIdAllocator::new();
        let mut paused = allocator
            .allocate_incident(IncidentSeed::completion_loss(
                event(72),
                RecoveryIncidentState::Active,
            ))
            .unwrap();
        assert!(paused.pause());
        let id = paused.id();
        let budget = paused.budget();
        let resumed = table_f(
            RecoveryWinner::new(LifecycleKind::DPMS).with_dpms_target(DpmsTarget::On),
            paused,
            event(73),
        );
        let current = resumed.fate.current_incident.unwrap();
        assert_eq!(current.id(), id);
        assert_eq!(current.state(), RecoveryIncidentState::Active);
        assert_eq!(current.budget(), budget);
        assert_eq!(resumed.fate.new_incident, None);
        assert!(!resumed.allocate_fresh_id);

        let mut spent = allocator
            .allocate_incident(IncidentSeed::completion_loss(
                event(74),
                RecoveryIncidentState::Active,
            ))
            .unwrap();
        assert!(spent.begin_attempt());
        assert!(spent.pause());
        let resumed_spent = table_f(
            RecoveryWinner::new(LifecycleKind::DPMS).with_dpms_target(DpmsTarget::On),
            spent,
            event(75),
        );
        let spent_current = resumed_spent.fate.current_incident.unwrap();
        assert_eq!(spent_current.id(), spent.id());
        assert_eq!(spent_current.state(), RecoveryIncidentState::Attempting);
        assert_eq!(spent_current.budget(), RecoveryAttemptBudget::Spent);

        let mut desired = LifecycleDesired::<u32>::default();
        desired.project(event(76), DesiredIntent::Dpms { level: 0, epoch: 1 });
        assert!(desired.set_disposition(
            event(76),
            Disposition::Deferred(Prerequisite::ReadinessClosed)
        ));
        assert_eq!(
            desired.disposition(event(76)),
            Some(Disposition::Deferred(Prerequisite::ReadinessClosed))
        );
        assert_eq!(
            resumed.fate.current_incident.unwrap().representative(),
            Some(event(72))
        );
    }

    #[test]
    fn c0_3a_one_attempt_per_incident() {
        let mut allocator = RecoveryIdAllocator::new();
        let mut active = allocator
            .allocate_incident(IncidentSeed::completion_loss(
                event(90),
                RecoveryIncidentState::Active,
            ))
            .unwrap();
        assert!(active.begin_attempt());
        let mut failed = active.resolve(RecoveryResolution::Failed).incident.unwrap();
        assert_eq!(failed.state(), RecoveryIncidentState::RecoveryFailed);
        assert_eq!(failed.budget(), RecoveryAttemptBudget::Spent);
        assert!(!failed.begin_attempt());

        let dpms_on = table_f(
            RecoveryWinner::new(LifecycleKind::DPMS).with_dpms_target(DpmsTarget::On),
            failed,
            event(91),
        );
        assert_eq!(dpms_on.fate.current_incident, Some(failed));
        assert_eq!(dpms_on.fate.new_incident, None);
        assert!(!dpms_on.allocate_fresh_id);
        assert_eq!(
            dpms_on.incident_state,
            Some(RecoveryIncidentState::RecoveryFailed)
        );
        let mut after_dpms = dpms_on.fate.current_incident.unwrap();
        for trigger in RecoveryAttemptTrigger::ALL {
            assert!(!after_dpms.request_attempt(trigger), "{trigger:?}");
        }
        for event_id in [event(92), event(93), event(94)] {
            let later_loss = table_f(
                RecoveryWinner::new(LifecycleKind::NormalRecovery),
                failed,
                event_id,
            );
            assert_eq!(
                later_loss.fate.event_disposition,
                Some((
                    event_id,
                    Disposition::Invalidated(InvalidationReason::RecoveryFailed)
                ))
            );
            assert_eq!(later_loss.fate.new_incident, None);
            assert!(!later_loss.allocate_fresh_id);
        }
    }
}
