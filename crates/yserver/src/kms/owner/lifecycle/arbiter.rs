//! Pure per-device lifecycle arbitration from C.0 §6.4 and REC-4.

use super::{
    CompletionUnknownRow, DesiredField, DesiredIntent, DeviceLifecycleState, Disposition,
    DpmsTarget, IncidentOrigin, IncidentSeed, LifecycleDesired, LifecycleEpochId, LifecycleEventId,
    LifecycleKind, LifecycleTransitionId, OutputProjection, Prerequisite, RecoveryAttemptTrigger,
    RecoveryFate, RecoveryIncident, RecoveryResolution, RecoveryWinner, TableFOutcome,
    TableUOutcome, TransitionTag, WorkTag, dpms_target_for_level, table_f, table_u,
};

/// Commit progress known by the arbiter. A submitted request cannot be
/// cancelled as if it had never reached the executor.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub enum CommitProgress {
    NotSubmitted,
    Submitting,
    Accepted,
}

/// The safety obligations that the winner must acknowledge before physical
/// lifecycle work is allowed to advance.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub enum LifecycleReceipt {
    AdmissionClosed,
    PreSubmitWorkCancelled,
    PresentsTerminalized,
    QuarantineTransferred,
}

impl LifecycleReceipt {
    /// Every receipt obligation, for generated exhaustive tests.
    pub const ALL: [Self; 4] = [
        Self::AdmissionClosed,
        Self::PreSubmitWorkCancelled,
        Self::PresentsTerminalized,
        Self::QuarantineTransferred,
    ];

    const fn bit(self) -> u8 {
        match self {
            Self::AdmissionClosed => 1 << 0,
            Self::PreSubmitWorkCancelled => 1 << 1,
            Self::PresentsTerminalized => 1 << 2,
            Self::QuarantineTransferred => 1 << 3,
        }
    }

    const fn all_bits() -> u8 {
        let mut bits = 0;
        let mut index = 0;
        while index < Self::ALL.len() {
            bits |= Self::ALL[index].bit();
            index += 1;
        }
        bits
    }
}

/// Whether a safety receipt proves its obligation or leaves the winner fenced.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub enum LifecycleReceiptResult {
    Succeeded,
    Failed,
}

/// Current phase of the one per-device transition.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub enum LifecycleTransitionPhase {
    AwaitingReceipts,
    PhysicalReady,
    PhysicallyFenced,
    AwaitingTerminal,
}

/// The sole current `REC-4` transition. Its identity is allocated by the
/// arbiter; `LifecycleTransitionId` itself intentionally has no public `next`.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub struct LifecycleTransition {
    pub id: LifecycleTransitionId,
    pub kind: LifecycleKind,
    pub phase: LifecycleTransitionPhase,
    commit_progress: CommitProgress,
    commit_epoch: LifecycleEpochId,
    terminal_wait_requested: bool,
    acknowledged: u8,
    failed: u8,
}

impl LifecycleTransition {
    const fn new(id: LifecycleTransitionId, kind: LifecycleKind, epoch: LifecycleEpochId) -> Self {
        Self {
            id,
            kind,
            phase: LifecycleTransitionPhase::AwaitingReceipts,
            commit_progress: CommitProgress::NotSubmitted,
            commit_epoch: epoch,
            terminal_wait_requested: false,
            acknowledged: 0,
            failed: 0,
        }
    }

    pub const fn commit_progress(self) -> CommitProgress {
        self.commit_progress
    }

    const fn receipts_complete(self) -> bool {
        self.acknowledged == LifecycleReceipt::all_bits() && self.failed == 0
    }

    const fn physically_fenced(self) -> bool {
        self.failed != 0
    }
}

/// Input from the coordinator, driver, or an acknowledged executor result.
/// The incarnation parameter stays opaque to this pure module.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub enum ArbiterInput<I> {
    LifecycleEvent {
        event_id: LifecycleEventId,
        intent: DesiredIntent,
    },
    Receipt {
        tag: TransitionTag<I>,
        receipt: LifecycleReceipt,
        result: LifecycleReceiptResult,
    },
    CommitProgress {
        tag: TransitionTag<I>,
        progress: CommitProgress,
    },
    CommitOutcome {
        tag: TransitionTag<I>,
        outcome: LifecycleCommitOutcome,
    },
    /// A completion-loss event intentionally has no work tag: C.0 §10 selects
    /// the row active when the loss is observed, even if ordinary work caused
    /// it. Boundary attempts supply the fresh Task 3 recovery id they own.
    CompletionUnknown {
        event_id: LifecycleEventId,
        boundary_recovery_id: Option<super::RecoveryId>,
    },
    RecoveryIncidentAllocated {
        event_id: LifecycleEventId,
        incident: RecoveryIncident,
    },
    /// Progress of the one Table U/F-authorized recovery attempt. The tag is
    /// the recovery transition, not the completion-loss event's old work tag.
    RecoveryAttempt {
        tag: TransitionTag<I>,
        outcome: RecoveryAttemptOutcome,
    },
    DeviceStateChanged(DeviceLifecycleState),
    /// A read-only seat ownership observation from the existing VT path.
    /// It updates the external prerequisite and retries bounded convergence,
    /// without projecting a VT release/acquire transition of its own.
    SeatTargetObserved {
        target: super::SeatTarget,
        epoch: u64,
    },
    /// The 3d fd-family barrier discharged a quarantine fence left by a
    /// failed or late safety receipt.
    FdFamilyBarrierCleared {
        tag: TransitionTag<I>,
    },
    OrdinaryReply {
        tag: WorkTag<I>,
    },
}

/// The attempt-start acknowledgement and its one terminal result.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub enum RecoveryAttemptOutcome {
    /// Accepted only after the authorized reap and only for an available id.
    Started(RecoveryAttemptTrigger),
    Qualified,
    FailedOrUnknown,
}

/// Terminal result at the C.0 result boundary.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub enum LifecycleCommitOutcome {
    Completed {
        /// DPMS is `Applied` only after every still-current output projection
        /// on this device has retired.
        dpms_projections_retired: bool,
    },
    Rejected {
        /// `Some(generation)` is an object-combination topology latch;
        /// `None` closes readiness until another lifecycle transition.
        topology_latched_generation: Option<u64>,
    },
}

/// Typed actions for the 3a-ii driver. Actions assert obligations only; the
/// driver returns receipts or terminal results before advancement.
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum LifecycleAction<I> {
    CloseAdmission(WorkTag<I>),
    ReopenAdmission(WorkTag<I>),
    StopAliasCreation(TransitionTag<I>),
    CancelPreSubmitWork(TransitionTag<I>),
    AwaitTerminalState(TransitionTag<I>),
    TerminalizePresents(TransitionTag<I>),
    TransferQuarantineToWinner(TransitionTag<I>),
    ReleaseSeat(WorkTag<I>),
    WithdrawOutputs(WorkTag<I>),
    TerminalizeProtocolWork(WorkTag<I>),
    EpochAdvanced(LifecycleEpochId),
    PhysicalAdvanceAllowed(TransitionTag<I>),
    DispositionChanged {
        event_id: LifecycleEventId,
        disposition: Disposition,
    },
    AllocateRecoveryIncident {
        event_id: LifecycleEventId,
        seed: IncidentSeed,
    },
    RecoveryTableF(TableFOutcome),
    CompletionLossTableU {
        row: CompletionUnknownRow,
        outcome: TableUOutcome,
    },
    CompletionBarriersRequired(TableUOutcome),
    ReceiptFailed {
        transition_id: LifecycleTransitionId,
        receipt: LifecycleReceipt,
    },
    StaleReceiptIgnored {
        transition_id: LifecycleTransitionId,
        receipt: LifecycleReceipt,
        result: LifecycleReceiptResult,
    },
    StaleResultIgnored {
        transition_id: Option<LifecycleTransitionId>,
    },
    OrdinaryReplyCurrent,
    TransitionIdExhausted,
    EpochExhausted,
    MissingBoundaryRecoveryId(LifecycleKind),
}

/// Per-device arbiter. The desired snapshot and every counter are bounded by
/// protocol fields, output domain, or current lifecycle state, never by event
/// history.
#[derive(Debug, Eq, PartialEq)]
pub struct LifecycleArbiter<O, I> {
    incarnation: I,
    desired: LifecycleDesired<O>,
    state: DeviceLifecycleState,
    epoch: LifecycleEpochId,
    transition: Option<LifecycleTransition>,
    next_transition_id: Option<u64>,
    last_transition_id: Option<LifecycleTransitionId>,
    recovery: Option<RecoveryIncident>,
    admission_open: bool,
    coalesced_epoch_bumped: bool,
    topology_latch: Option<u64>,
    pending_recovery_allocation: Option<(LifecycleEventId, IncidentSeed)>,
    fd_family_fenced: bool,
}

impl<O: Ord, I: Clone + Eq> LifecycleArbiter<O, I> {
    pub fn new(incarnation: I) -> Self {
        Self {
            incarnation,
            desired: LifecycleDesired::default(),
            state: DeviceLifecycleState::Ready,
            epoch: LifecycleEpochId::first(),
            transition: None,
            next_transition_id: Some(1),
            last_transition_id: None,
            recovery: None,
            admission_open: true,
            coalesced_epoch_bumped: false,
            topology_latch: None,
            pending_recovery_allocation: None,
            fd_family_fenced: false,
        }
    }

    pub fn desired(&self) -> &LifecycleDesired<O> {
        &self.desired
    }

    pub const fn state(&self) -> DeviceLifecycleState {
        self.state
    }

    pub const fn epoch(&self) -> LifecycleEpochId {
        self.epoch
    }

    pub const fn transition(&self) -> Option<LifecycleTransition> {
        self.transition
    }

    /// Tag a current transition for the driver and its acknowledged inputs.
    pub fn transition_tag(&self) -> Option<TransitionTag<I>> {
        self.transition
            .map(|transition| self.tag_for(transition.id, self.epoch))
    }

    pub const fn recovery(&self) -> Option<RecoveryIncident> {
        self.recovery
    }

    pub const fn admission_open(&self) -> bool {
        self.admission_open
    }

    pub fn add_protocol_output(&mut self, output: O) -> Option<OutputProjection> {
        self.desired.add_protocol_output(output)
    }

    pub(crate) fn observe_seat_target(&mut self, target: super::SeatTarget, epoch: u64) {
        self.desired.observe_seat_target(target, epoch);
    }

    /// Remove one stable protocol output projection from this device.
    pub fn remove_protocol_output(&mut self, output: &O) -> Option<super::OutputProjectionRemoval> {
        self.desired.remove_protocol_output(output)
    }

    /// Ordinary work always carries the current lifecycle epoch and no
    /// transition id, including after a previous transition completed.
    pub fn ordinary_work_tag(&self) -> Option<WorkTag<I>> {
        (self.admission_open && self.state == DeviceLifecycleState::Ready)
            .then(|| WorkTag::ordinary(self.incarnation.clone(), self.epoch))
    }

    /// Apply one projected event or acknowledged outcome and return driver
    /// actions in their required ordering.
    pub fn apply(&mut self, input: ArbiterInput<I>) -> Vec<LifecycleAction<I>> {
        match input {
            ArbiterInput::LifecycleEvent { event_id, intent } => self.apply_event(event_id, intent),
            ArbiterInput::Receipt {
                tag,
                receipt,
                result,
            } => self.apply_receipt(tag, receipt, result),
            ArbiterInput::CommitProgress { tag, progress } => {
                self.apply_commit_progress(tag, progress)
            }
            ArbiterInput::CommitOutcome { tag, outcome } => self.apply_commit_outcome(tag, outcome),
            ArbiterInput::CompletionUnknown {
                event_id,
                boundary_recovery_id,
            } => self.apply_completion_unknown(event_id, boundary_recovery_id),
            ArbiterInput::RecoveryIncidentAllocated { event_id, incident } => {
                self.apply_allocated_incident(event_id, incident)
            }
            ArbiterInput::RecoveryAttempt { tag, outcome } => {
                self.apply_recovery_attempt(tag, outcome)
            }
            ArbiterInput::DeviceStateChanged(state) => {
                self.state = state;
                if state == DeviceLifecycleState::Ready && self.transition.is_none() {
                    self.admission_open = true;
                }
                let mut actions = self.converge();
                if state == DeviceLifecycleState::Ready
                    && self.transition.is_none()
                    && self.admission_open
                {
                    actions.push(LifecycleAction::ReopenAdmission(self.current_work_tag()));
                }
                actions
            }
            ArbiterInput::SeatTargetObserved { target, epoch } => {
                self.desired.observe_seat_target(target, epoch);
                self.converge()
            }
            ArbiterInput::FdFamilyBarrierCleared { tag } => {
                if !self.is_current_transition_tag(&tag) {
                    return vec![LifecycleAction::StaleResultIgnored {
                        transition_id: Some(tag.transition),
                    }];
                }
                self.fd_family_fenced = false;
                let ready_id = self.transition.and_then(|transition| {
                    (transition.failed == 0 && transition.receipts_complete())
                        .then_some(transition.id)
                });
                if let Some(transition_id) = ready_id {
                    self.transition
                        .as_mut()
                        .expect("transition remains active")
                        .phase = LifecycleTransitionPhase::PhysicalReady;
                    return vec![LifecycleAction::PhysicalAdvanceAllowed(
                        self.tag_for(transition_id, self.epoch),
                    )];
                }
                Vec::new()
            }
            ArbiterInput::OrdinaryReply { tag } => {
                if self.is_current_ordinary_tag(&tag) {
                    vec![LifecycleAction::OrdinaryReplyCurrent]
                } else {
                    vec![LifecycleAction::StaleResultIgnored {
                        transition_id: tag.transition.or(self.last_transition_id),
                    }]
                }
            }
        }
    }

    fn apply_event(
        &mut self,
        event_id: LifecycleEventId,
        intent: DesiredIntent,
    ) -> Vec<LifecycleAction<I>> {
        let kind = intent.kind();
        let mut actions = Vec::new();

        // Close admission before touching the desired snapshot or terminalizing
        // any replaced representative. If a transition already exists, this
        // carries that current id; the winner also republishes the close under
        // its own id when it is created.
        actions.push(LifecycleAction::CloseAdmission(self.current_work_tag()));
        self.admission_open = false;

        // Admission closes before the snapshot changes or any representative
        // is invalidated. The current transition already owns the close if it
        // exists; a new transition publishes a tagged close below.
        if let Some(active) = self.transition {
            if kind.outranks(active.kind) {
                let old_tag = self.tag_for(active.id, self.epoch);
                self.force_epoch_bump(&mut actions);
                self.finish_abandoned_transition(active, old_tag, &mut actions);
            } else if kind == active.kind && !self.coalesced_epoch_bumped {
                self.force_epoch_bump(&mut actions);
                self.coalesced_epoch_bumped = true;
            }
        }

        let projection = self.desired.project(event_id, intent);
        for terminal in projection.terminalized {
            actions.push(LifecycleAction::DispositionChanged {
                event_id: terminal.event_id,
                disposition: terminal.disposition,
            });
        }
        if let Some(disposition) = projection.event_disposition {
            actions.push(LifecycleAction::DispositionChanged {
                event_id,
                disposition,
            });
            return actions;
        }

        if kind == LifecycleKind::Shutdown {
            self.state = DeviceLifecycleState::Quiescing;
        }

        if self.state == DeviceLifecycleState::Poisoned && kind == LifecycleKind::DPMS {
            self.logical_poisoned_dpms(event_id, &mut actions);
            return actions;
        }

        match self.transition {
            Some(active) if kind.outranks(active.kind) => {
                self.start_transition(kind, &mut actions);
            }
            Some(active) if kind == active.kind => {
                // REC-5 coalesces the latest target into the same transition.
                // Its safety receipts remain valid for this unchanged id, so
                // do not terminalize Presents or transfer quarantine twice.
                if active.commit_progress != CommitProgress::NotSubmitted
                    && !active.terminal_wait_requested
                {
                    if let Some(transition) = self.transition.as_mut() {
                        transition.terminal_wait_requested = true;
                    }
                    actions.push(LifecycleAction::AwaitTerminalState(
                        self.tag_for(active.id, active.commit_epoch),
                    ));
                }
                if active.commit_progress == CommitProgress::NotSubmitted
                    && active.receipts_complete()
                    && !self.fd_family_fenced
                {
                    actions.push(LifecycleAction::PhysicalAdvanceAllowed(
                        self.tag_for(active.id, self.epoch),
                    ));
                }
                self.apply_recovery_winner(kind, event_id, &mut actions);
            }
            Some(_) => {
                self.classify_prerequisite(event_id, kind, &mut actions);
            }
            None => {
                let blocked = self.classify_prerequisite(event_id, kind, &mut actions);
                if blocked {
                    if self.state == DeviceLifecycleState::Ready {
                        self.admission_open = true;
                        actions.push(LifecycleAction::ReopenAdmission(self.current_work_tag()));
                    }
                } else if self.transition.is_none() {
                    self.start_transition(kind, &mut actions);
                    self.apply_recovery_winner(kind, event_id, &mut actions);
                    self.emit_logical_obligations(kind, &mut actions);
                }
            }
        }
        actions
    }

    fn finish_abandoned_transition(
        &mut self,
        active: LifecycleTransition,
        old_tag: TransitionTag<I>,
        actions: &mut Vec<LifecycleAction<I>>,
    ) {
        if active.physically_fenced() {
            self.fd_family_fenced = true;
        }
        match active.commit_progress {
            CommitProgress::NotSubmitted => {
                actions.push(LifecycleAction::CancelPreSubmitWork(old_tag));
            }
            CommitProgress::Submitting | CommitProgress::Accepted => {
                actions.push(LifecycleAction::AwaitTerminalState(old_tag));
            }
        }
        self.transition = None;
        self.coalesced_epoch_bumped = false;
    }

    fn start_transition(&mut self, kind: LifecycleKind, actions: &mut Vec<LifecycleAction<I>>) {
        let Some(id) = self.allocate_transition_id() else {
            actions.push(LifecycleAction::TransitionIdExhausted);
            return;
        };
        self.last_transition_id = Some(id);
        self.transition = Some(LifecycleTransition::new(id, kind, self.epoch));
        self.admission_open = false;
        self.coalesced_epoch_bumped = false;
        self.emit_safety_requests(actions);
    }

    fn allocate_transition_id(&mut self) -> Option<LifecycleTransitionId> {
        let raw = self.next_transition_id?;
        self.next_transition_id = raw.checked_add(1);
        Some(LifecycleTransitionId::from_raw(raw))
    }

    fn emit_safety_requests(&self, actions: &mut Vec<LifecycleAction<I>>) {
        let Some(transition) = self.transition else {
            return;
        };
        let tag = self.tag_for(transition.id, self.epoch);
        actions.push(LifecycleAction::CloseAdmission(tag.clone().as_work_tag()));
        actions.push(LifecycleAction::StopAliasCreation(tag.clone()));
        actions.push(LifecycleAction::CancelPreSubmitWork(tag.clone()));
        if matches!(
            transition.commit_progress,
            CommitProgress::Submitting | CommitProgress::Accepted
        ) {
            actions.push(LifecycleAction::AwaitTerminalState(tag.clone()));
        }
        actions.push(LifecycleAction::TerminalizePresents(tag.clone()));
        actions.push(LifecycleAction::TransferQuarantineToWinner(tag));
    }

    fn emit_logical_obligations(&self, kind: LifecycleKind, actions: &mut Vec<LifecycleAction<I>>) {
        let Some(transition) = self.transition else {
            return;
        };
        let tag = self.tag_for(transition.id, self.epoch).as_work_tag();
        match kind {
            LifecycleKind::VTRelease => actions.push(LifecycleAction::ReleaseSeat(tag)),
            LifecycleKind::DeviceRemoved => {
                actions.push(LifecycleAction::WithdrawOutputs(tag.clone()));
                actions.push(LifecycleAction::TerminalizeProtocolWork(tag));
            }
            LifecycleKind::Shutdown => {
                actions.push(LifecycleAction::WithdrawOutputs(tag.clone()));
                actions.push(LifecycleAction::TerminalizeProtocolWork(tag.clone()));
                actions.push(LifecycleAction::ReleaseSeat(tag));
            }
            _ => {}
        }
    }

    fn apply_recovery_winner(
        &mut self,
        kind: LifecycleKind,
        event_id: LifecycleEventId,
        actions: &mut Vec<LifecycleAction<I>>,
    ) {
        let Some(incident) = self.recovery else {
            return;
        };
        let recovery_required = matches!(
            self.state,
            DeviceLifecycleState::Poisoned
                | DeviceLifecycleState::RecoveryFailed
                | DeviceLifecycleState::Recovering(_)
        ) || matches!(
            kind,
            LifecycleKind::DeviceAddedOrReplaced
                | LifecycleKind::VTAcquire
                | LifecycleKind::AdministrativeReprobe
                | LifecycleKind::IdentityChangingHotplug
        );
        let mut winner = RecoveryWinner::new(kind).with_dpms_target(self.current_dpms_target());
        if recovery_required {
            winner = winner.requiring_recovery();
        }
        let fate = table_f(winner, incident, event_id);
        self.apply_recovery_fate(fate.fate, actions);
        actions.push(LifecycleAction::RecoveryTableF(fate));
        if let Some(seed) = fate.fate.new_incident {
            self.pending_recovery_allocation = Some((event_id, seed));
            actions.push(LifecycleAction::AllocateRecoveryIncident { event_id, seed });
        }
    }

    fn apply_recovery_fate(&mut self, fate: RecoveryFate, actions: &mut Vec<LifecycleAction<I>>) {
        if let Some((event_id, disposition)) = fate.representative_disposition {
            self.set_disposition(event_id, disposition, actions);
        }
        if let Some((event_id, disposition)) = fate.event_disposition {
            self.set_disposition(event_id, disposition, actions);
        }
        if let Some(invalidation) = fate.invalidated_incident
            && let Some(representative) = self.recovery.and_then(RecoveryIncident::representative)
            && self
                .desired
                .disposition(representative)
                .is_some_and(Disposition::is_terminal)
        {
            self.desired
                .clear_recovery_incident(invalidation.recovery_id);
        }
        self.recovery = fate.current_incident;
    }

    fn logical_poisoned_dpms(
        &mut self,
        event_id: LifecycleEventId,
        actions: &mut Vec<LifecycleAction<I>>,
    ) {
        self.set_disposition(
            event_id,
            Disposition::Deferred(Prerequisite::ReadinessClosed),
            actions,
        );
        if let Some(incident) = self.recovery {
            let outcome = table_f(
                RecoveryWinner::new(LifecycleKind::DPMS)
                    .with_dpms_target(self.current_dpms_target()),
                incident,
                event_id,
            );
            self.apply_recovery_fate(outcome.fate, actions);
            actions.push(LifecycleAction::RecoveryTableF(outcome));
        }
        self.ensure_recovery_transition(actions);
    }

    fn current_dpms_target(&self) -> DpmsTarget {
        dpms_target_for_level(self.desired.protocol_dpms_level())
            .expect("DPMS levels are validated by the coordinator")
    }

    fn classify_prerequisite(
        &mut self,
        event_id: LifecycleEventId,
        kind: LifecycleKind,
        actions: &mut Vec<LifecycleAction<I>>,
    ) -> bool {
        let prerequisite = match kind {
            LifecycleKind::DeviceAddedOrReplaced
                if self.desired.seat_target() != Some(super::SeatTarget::Owned) =>
            {
                Some(Prerequisite::SeatReleased)
            }
            LifecycleKind::VTAcquire if self.desired.device_presence() != Some(true) => {
                Some(Prerequisite::DeviceAbsent)
            }
            LifecycleKind::DPMS if self.state == DeviceLifecycleState::Poisoned => {
                Some(Prerequisite::ReadinessClosed)
            }
            LifecycleKind::DPMS
                if self.desired.seat_target() == Some(super::SeatTarget::Released) =>
            {
                Some(Prerequisite::SeatReleased)
            }
            LifecycleKind::DPMS if self.state != DeviceLifecycleState::Ready => {
                Some(Prerequisite::ReadinessClosed)
            }
            LifecycleKind::DPMS
                if self.topology_latch.is_some_and(|generation| {
                    self.desired
                        .discovery_epoch()
                        .is_some_and(|current| current != generation)
                }) =>
            {
                None
            }
            LifecycleKind::DPMS if let Some(generation) = self.topology_latch => {
                Some(Prerequisite::TopologyLatched(generation))
            }
            _ => None,
        };
        if let Some(prerequisite) = prerequisite {
            self.set_disposition(event_id, Disposition::Deferred(prerequisite), actions);
            true
        } else {
            false
        }
    }

    fn apply_receipt(
        &mut self,
        tag: TransitionTag<I>,
        receipt: LifecycleReceipt,
        result: LifecycleReceiptResult,
    ) -> Vec<LifecycleAction<I>> {
        if !self.matches_current_transition_id(&tag) {
            if result == LifecycleReceiptResult::Failed {
                self.fd_family_fenced = true;
            }
            return vec![LifecycleAction::StaleReceiptIgnored {
                transition_id: tag.transition,
                receipt,
                result,
            }];
        }
        let transition = self.transition.as_mut().expect("current transition exists");
        match result {
            LifecycleReceiptResult::Succeeded => transition.acknowledged |= receipt.bit(),
            LifecycleReceiptResult::Failed => {
                transition.failed |= receipt.bit();
            }
        }
        if transition.physically_fenced() {
            transition.phase = LifecycleTransitionPhase::PhysicallyFenced;
        }
        if transition.receipts_complete()
            && !self.fd_family_fenced
            && transition.phase != LifecycleTransitionPhase::PhysicalReady
        {
            transition.phase = LifecycleTransitionPhase::PhysicalReady;
            vec![LifecycleAction::PhysicalAdvanceAllowed(tag)]
        } else if transition.receipts_complete() && self.fd_family_fenced {
            transition.phase = LifecycleTransitionPhase::PhysicallyFenced;
            Vec::new()
        } else if result == LifecycleReceiptResult::Failed {
            vec![LifecycleAction::ReceiptFailed {
                transition_id: tag.transition,
                receipt,
            }]
        } else {
            Vec::new()
        }
    }

    fn apply_commit_progress(
        &mut self,
        tag: TransitionTag<I>,
        progress: CommitProgress,
    ) -> Vec<LifecycleAction<I>> {
        if !self.is_current_transition_tag(&tag) {
            if self.matches_current_transition_id(&tag)
                && let Some(transition) = self.transition.as_mut()
            {
                transition.commit_progress = progress;
                transition.commit_epoch = tag.lifecycle_epoch;
                if matches!(
                    progress,
                    CommitProgress::Submitting | CommitProgress::Accepted
                ) {
                    transition.phase = LifecycleTransitionPhase::AwaitingTerminal;
                }
            }
            return vec![LifecycleAction::StaleResultIgnored {
                transition_id: Some(tag.transition),
            }];
        }
        if let Some(transition) = self.transition.as_mut() {
            transition.commit_progress = progress;
            transition.commit_epoch = tag.lifecycle_epoch;
            if matches!(
                progress,
                CommitProgress::Submitting | CommitProgress::Accepted
            ) {
                transition.phase = LifecycleTransitionPhase::AwaitingTerminal;
            }
        }
        Vec::new()
    }

    fn apply_commit_outcome(
        &mut self,
        tag: TransitionTag<I>,
        outcome: LifecycleCommitOutcome,
    ) -> Vec<LifecycleAction<I>> {
        if !self.is_current_transition_tag(&tag) {
            if self.matches_current_transition_id(&tag) {
                let ready = if let Some(transition) = self.transition.as_mut() {
                    transition.commit_progress = CommitProgress::NotSubmitted;
                    transition.terminal_wait_requested = false;
                    transition.receipts_complete() && !self.fd_family_fenced
                } else {
                    false
                };
                let mut actions = vec![LifecycleAction::StaleResultIgnored {
                    transition_id: Some(tag.transition),
                }];
                if ready {
                    if let Some(transition) = self.transition.as_mut() {
                        transition.phase = LifecycleTransitionPhase::PhysicalReady;
                    }
                    actions.push(LifecycleAction::PhysicalAdvanceAllowed(
                        self.tag_for(tag.transition, self.epoch),
                    ));
                }
                return actions;
            }
            return vec![LifecycleAction::StaleResultIgnored {
                transition_id: Some(tag.transition),
            }];
        }
        let active = self.transition.expect("current transition exists");
        let mut actions = Vec::new();
        match outcome {
            LifecycleCommitOutcome::Completed {
                dpms_projections_retired,
            } => {
                if active.kind != LifecycleKind::DPMS || dpms_projections_retired {
                    self.mark_active_representative_applied(active, &mut actions);
                } else if let Some(representative) =
                    self.desired.representative(field_for_kind(active.kind))
                {
                    self.set_disposition(
                        representative.event_id,
                        Disposition::Deferred(Prerequisite::ReadinessClosed),
                        &mut actions,
                    );
                }
                self.state = match active.kind {
                    LifecycleKind::Shutdown | LifecycleKind::VTRelease => {
                        DeviceLifecycleState::Quiescing
                    }
                    LifecycleKind::DeviceRemoved => DeviceLifecycleState::Removed,
                    LifecycleKind::DeviceAddedOrReplaced
                    | LifecycleKind::VTAcquire
                    | LifecycleKind::AdministrativeReprobe
                    | LifecycleKind::IdentityChangingHotplug
                    | LifecycleKind::TopologyRebuild
                    | LifecycleKind::DPMS
                    | LifecycleKind::NormalRecovery => DeviceLifecycleState::Ready,
                };
                if matches!(
                    active.kind,
                    LifecycleKind::IdentityChangingHotplug | LifecycleKind::TopologyRebuild
                ) && self
                    .desired
                    .representative(DesiredField::Topology)
                    .is_some_and(|representative| representative.kind == active.kind)
                    && let Some(generation) = self.desired.discovery_epoch()
                {
                    self.desired.clear_topology_dirty(generation);
                }
                self.transition = None;
                self.admission_open = self.state == DeviceLifecycleState::Ready;
                self.coalesced_epoch_bumped = false;
                actions.extend(self.converge());
                if self.admission_open && self.transition.is_none() {
                    actions.push(LifecycleAction::ReopenAdmission(self.current_work_tag()));
                }
            }
            LifecycleCommitOutcome::Rejected {
                topology_latched_generation,
            } => {
                if let Some(generation) = topology_latched_generation {
                    self.topology_latch = Some(generation);
                    self.state = DeviceLifecycleState::Ready;
                    self.admission_open = true;
                    if let Some(representative) =
                        self.desired.representative(field_for_kind(active.kind))
                    {
                        self.set_disposition(
                            representative.event_id,
                            Disposition::Deferred(Prerequisite::TopologyLatched(generation)),
                            &mut actions,
                        );
                    }
                } else {
                    self.state = DeviceLifecycleState::Quiescing;
                    self.admission_open = false;
                    if let Some(representative) =
                        self.desired.representative(field_for_kind(active.kind))
                    {
                        self.set_disposition(
                            representative.event_id,
                            Disposition::Deferred(Prerequisite::ReadinessClosed),
                            &mut actions,
                        );
                    }
                }
                self.transition = None;
                self.coalesced_epoch_bumped = false;
                actions.extend(self.converge());
                if self.admission_open && self.transition.is_none() {
                    actions.push(LifecycleAction::ReopenAdmission(self.current_work_tag()));
                }
            }
        }
        actions
    }

    fn mark_active_representative_applied(
        &mut self,
        active: LifecycleTransition,
        actions: &mut Vec<LifecycleAction<I>>,
    ) {
        let Some(representative) = self.desired.representative(field_for_kind(active.kind)) else {
            return;
        };
        if representative.kind != active.kind {
            return;
        }
        if representative.disposition.is_none()
            || matches!(representative.disposition, Some(Disposition::Deferred(_)))
        {
            let disposition = Disposition::Applied(active.id);
            if self
                .desired
                .resolve_deferred(representative.event_id, disposition)
                || self
                    .desired
                    .set_disposition(representative.event_id, disposition)
            {
                actions.push(LifecycleAction::DispositionChanged {
                    event_id: representative.event_id,
                    disposition,
                });
            }
        }
    }

    fn apply_completion_unknown(
        &mut self,
        event_id: LifecycleEventId,
        boundary_recovery_id: Option<super::RecoveryId>,
    ) -> Vec<LifecycleAction<I>> {
        let active_kind = self
            .transition
            .map(|transition| transition.kind)
            .unwrap_or(LifecycleKind::NormalRecovery);
        let row = match active_kind {
            LifecycleKind::Shutdown => CompletionUnknownRow::Shutdown,
            LifecycleKind::DeviceRemoved => CompletionUnknownRow::DeviceRemoved,
            LifecycleKind::VTRelease => CompletionUnknownRow::VTRelease,
            LifecycleKind::DeviceAddedOrReplaced => match boundary_recovery_id {
                Some(recovery_id) => CompletionUnknownRow::DeviceAddedOrReplaced { recovery_id },
                None => return vec![LifecycleAction::MissingBoundaryRecoveryId(active_kind)],
            },
            LifecycleKind::VTAcquire => match boundary_recovery_id {
                Some(recovery_id) => CompletionUnknownRow::VTAcquire { recovery_id },
                None => return vec![LifecycleAction::MissingBoundaryRecoveryId(active_kind)],
            },
            LifecycleKind::AdministrativeReprobe => match boundary_recovery_id {
                Some(recovery_id) => CompletionUnknownRow::AdministrativeReprobe { recovery_id },
                None => return vec![LifecycleAction::MissingBoundaryRecoveryId(active_kind)],
            },
            LifecycleKind::IdentityChangingHotplug => match boundary_recovery_id {
                Some(recovery_id) => CompletionUnknownRow::IdentityChangingHotplug { recovery_id },
                None => return vec![LifecycleAction::MissingBoundaryRecoveryId(active_kind)],
            },
            LifecycleKind::TopologyRebuild => CompletionUnknownRow::TopologyRebuild,
            LifecycleKind::DPMS => CompletionUnknownRow::DPMS {
                target: self.current_dpms_target(),
            },
            LifecycleKind::NormalRecovery => CompletionUnknownRow::NormalLive,
        };
        let outcome = table_u(row, event_id, self.recovery);
        let mut actions = Vec::new();
        let work_tag = self.current_work_tag();
        self.transition = None;
        self.coalesced_epoch_bumped = false;
        if outcome.logical.release_seat {
            actions.push(LifecycleAction::ReleaseSeat(work_tag.clone()));
        }
        if outcome.logical.withdraw_protocol_work {
            actions.push(LifecycleAction::WithdrawOutputs(work_tag.clone()));
            actions.push(LifecycleAction::TerminalizeProtocolWork(work_tag));
        }
        if let Some((event, disposition)) = outcome.recovery.representative_disposition {
            self.set_disposition(event, disposition, &mut actions);
        }
        if let Some((event, disposition)) = outcome.recovery.event_disposition {
            let recorded = self.set_disposition(event, disposition, &mut actions);
            if event == event_id && !recorded {
                actions.push(LifecycleAction::DispositionChanged {
                    event_id: event,
                    disposition,
                });
            }
        }
        if matches!(outcome.logical.event, super::EventFate::Terminal { event_id: terminal_id, .. } if terminal_id == event_id)
            && outcome.recovery.event_disposition.is_none()
            && let super::EventFate::Terminal { disposition, .. } = outcome.logical.event
        {
            actions.push(LifecycleAction::DispositionChanged {
                event_id,
                disposition,
            });
        }
        if active_kind == LifecycleKind::DPMS
            && outcome.logical.poison_device
            && let Some(representative) = self.desired.representative(DesiredField::Dpms)
        {
            self.set_disposition(
                representative.event_id,
                Disposition::Deferred(Prerequisite::ReadinessClosed),
                &mut actions,
            );
        }
        if let Some(seed) = outcome.recovery.new_incident {
            self.pending_recovery_allocation = Some((event_id, seed));
            actions.push(LifecycleAction::AllocateRecoveryIncident { event_id, seed });
        }
        self.recovery = outcome.recovery.current_incident;
        if outcome.logical.poison_device {
            self.state = DeviceLifecycleState::Poisoned;
            self.admission_open = false;
        } else if outcome.logical.release_seat {
            self.state = DeviceLifecycleState::Quiescing;
            self.admission_open = false;
        }
        actions.push(LifecycleAction::CompletionLossTableU { row, outcome });
        actions.push(LifecycleAction::CompletionBarriersRequired(outcome));
        let recovery_transition_kind = if outcome.physical
            == super::PhysicalUnknownOutcome::TransferQuarantineToRebuildThenQualifiedInstall
        {
            LifecycleKind::TopologyRebuild
        } else {
            LifecycleKind::NormalRecovery
        };
        self.ensure_recovery_transition_as(recovery_transition_kind, &mut actions);
        actions
    }

    fn apply_allocated_incident(
        &mut self,
        event_id: LifecycleEventId,
        incident: RecoveryIncident,
    ) -> Vec<LifecycleAction<I>> {
        let Some((pending_event, _)) = self.pending_recovery_allocation else {
            return Vec::new();
        };
        if pending_event != event_id {
            return Vec::new();
        }
        self.pending_recovery_allocation = None;
        self.recovery = Some(incident);
        let mut actions = Vec::new();
        if matches!(incident.origin(), IncidentOrigin::CompletionLoss { .. }) {
            let projection = self.desired.project(
                event_id,
                DesiredIntent::NormalRecovery {
                    recovery_id: incident.id(),
                },
            );
            for terminal in projection.terminalized {
                actions.push(LifecycleAction::DispositionChanged {
                    event_id: terminal.event_id,
                    disposition: terminal.disposition,
                });
            }
            if let Some(disposition) = projection.event_disposition {
                actions.push(LifecycleAction::DispositionChanged {
                    event_id,
                    disposition,
                });
            }
        }
        self.ensure_recovery_transition(&mut actions);
        actions.extend(self.converge());
        actions
    }

    fn ensure_recovery_transition(&mut self, actions: &mut Vec<LifecycleAction<I>>) {
        self.ensure_recovery_transition_as(LifecycleKind::NormalRecovery, actions);
    }

    fn ensure_recovery_transition_as(
        &mut self,
        kind: LifecycleKind,
        actions: &mut Vec<LifecycleAction<I>>,
    ) {
        if self.transition.is_some()
            || !self.recovery.is_some_and(|incident| {
                incident.state() == super::RecoveryIncidentState::Active
                    && incident.budget() == super::RecoveryAttemptBudget::Available
            })
        {
            return;
        }

        self.start_transition(kind, actions);
    }

    fn apply_recovery_attempt(
        &mut self,
        tag: TransitionTag<I>,
        outcome: RecoveryAttemptOutcome,
    ) -> Vec<LifecycleAction<I>> {
        if !self.is_current_transition_tag(&tag) {
            return vec![LifecycleAction::StaleResultIgnored {
                transition_id: Some(tag.transition),
            }];
        }

        let Some(active) = self.transition else {
            return vec![LifecycleAction::StaleResultIgnored {
                transition_id: Some(tag.transition),
            }];
        };
        match outcome {
            RecoveryAttemptOutcome::Started(trigger) => {
                let authorized = active.phase == LifecycleTransitionPhase::PhysicalReady
                    && trigger == RecoveryAttemptTrigger::AfterReap
                    && self.recovery.is_some_and(|incident| {
                        incident.state() == super::RecoveryIncidentState::Active
                            && incident.budget() == super::RecoveryAttemptBudget::Available
                    });
                if !authorized {
                    return Vec::new();
                }
                let Some(incident) = self.recovery.as_mut() else {
                    return Vec::new();
                };
                if !incident.request_attempt(trigger) {
                    return Vec::new();
                }
                self.state = DeviceLifecycleState::Recovering(incident.id());
                Vec::new()
            }
            RecoveryAttemptOutcome::Qualified | RecoveryAttemptOutcome::FailedOrUnknown => {
                let Some(incident) = self.recovery else {
                    return Vec::new();
                };
                if self.state != DeviceLifecycleState::Recovering(incident.id())
                    || incident.state() != super::RecoveryIncidentState::Attempting
                {
                    return Vec::new();
                }

                let resolution = incident.resolve(match outcome {
                    RecoveryAttemptOutcome::Qualified => RecoveryResolution::Qualified(active.id),
                    RecoveryAttemptOutcome::FailedOrUnknown => RecoveryResolution::Failed,
                    RecoveryAttemptOutcome::Started(_) => unreachable!(),
                });
                let mut actions = Vec::new();
                if let Some((event_id, disposition)) = resolution.representative_disposition {
                    self.set_disposition(event_id, disposition, &mut actions);
                }
                self.transition = None;
                self.coalesced_epoch_bumped = false;
                match outcome {
                    RecoveryAttemptOutcome::Qualified => {
                        self.recovery = resolution.incident;
                        if resolution.representative_disposition.is_none()
                            || active.kind != LifecycleKind::NormalRecovery
                        {
                            self.mark_active_representative_applied(active, &mut actions);
                        }
                        self.state = DeviceLifecycleState::Ready;
                        self.admission_open = true;
                        self.desired.clear_recovery_incident(resolution.id);
                        actions.extend(self.converge());
                        if self.admission_open && self.transition.is_none() {
                            actions.push(LifecycleAction::ReopenAdmission(self.current_work_tag()));
                        }
                    }
                    RecoveryAttemptOutcome::FailedOrUnknown => {
                        self.recovery = resolution.incident;
                        self.state = DeviceLifecycleState::RecoveryFailed;
                        self.admission_open = false;
                    }
                    RecoveryAttemptOutcome::Started(_) => unreachable!(),
                }
                actions
            }
        }
    }

    fn converge(&mut self) -> Vec<LifecycleAction<I>> {
        if self.transition.is_some()
            || !self.admission_open && self.state == DeviceLifecycleState::Ready
            || self.state == DeviceLifecycleState::Poisoned
            || self.state == DeviceLifecycleState::Removed
            || self.state == DeviceLifecycleState::ShutdownExecutorStalled
        {
            return Vec::new();
        }

        let mut actions = Vec::new();
        let candidates: Vec<_> = self
            .desired
            .representatives()
            .iter()
            .copied()
            .filter(|representative| {
                representative.disposition.is_none() || self.deferred_is_runnable(*representative)
            })
            .collect();

        let mut runnable = Vec::new();
        for representative in candidates {
            if let Some(prerequisite) = self.prerequisite_for(representative.kind) {
                if !matches!(representative.disposition, Some(Disposition::Deferred(current)) if current == prerequisite)
                {
                    self.set_disposition(
                        representative.event_id,
                        Disposition::Deferred(prerequisite),
                        &mut actions,
                    );
                }
            } else {
                runnable.push(representative);
            }
        }

        if let Some(winner) = runnable
            .into_iter()
            .min_by_key(|representative| representative.kind.precedence())
        {
            self.start_transition(winner.kind, &mut actions);
            self.apply_recovery_winner(winner.kind, winner.event_id, &mut actions);
            self.emit_logical_obligations(winner.kind, &mut actions);
        }
        actions
    }

    fn deferred_is_runnable(&self, representative: super::Representative) -> bool {
        match representative.disposition {
            Some(Disposition::Deferred(Prerequisite::SeatReleased)) => {
                self.desired.seat_target() == Some(super::SeatTarget::Owned)
            }
            Some(Disposition::Deferred(Prerequisite::DeviceAbsent)) => {
                self.desired.device_presence() == Some(true)
            }
            Some(Disposition::Deferred(Prerequisite::TopologyLatched(generation))) => self
                .desired
                .discovery_epoch()
                .is_some_and(|current| current != generation),
            Some(Disposition::Deferred(Prerequisite::ReadinessClosed)) => {
                self.state == DeviceLifecycleState::Ready
            }
            _ => false,
        }
    }

    fn prerequisite_for(&self, kind: LifecycleKind) -> Option<Prerequisite> {
        match kind {
            LifecycleKind::DeviceAddedOrReplaced
                if self.desired.seat_target() != Some(super::SeatTarget::Owned) =>
            {
                Some(Prerequisite::SeatReleased)
            }
            LifecycleKind::VTAcquire if self.desired.device_presence() != Some(true) => {
                Some(Prerequisite::DeviceAbsent)
            }
            LifecycleKind::DPMS if self.state != DeviceLifecycleState::Ready => {
                Some(Prerequisite::ReadinessClosed)
            }
            LifecycleKind::DPMS
                if self.desired.seat_target() == Some(super::SeatTarget::Released) =>
            {
                Some(Prerequisite::SeatReleased)
            }
            LifecycleKind::DPMS if let Some(generation) = self.topology_latch => self
                .desired
                .discovery_epoch()
                .filter(|current| *current == generation)
                .map(Prerequisite::TopologyLatched)
                .or_else(|| {
                    self.desired
                        .discovery_epoch()
                        .is_none()
                        .then_some(Prerequisite::TopologyLatched(generation))
                }),
            _ => None,
        }
    }

    fn set_disposition(
        &mut self,
        event_id: LifecycleEventId,
        disposition: Disposition,
        actions: &mut Vec<LifecycleAction<I>>,
    ) -> bool {
        let changed = if disposition.is_terminal() {
            self.desired.resolve_deferred(event_id, disposition)
                || self.desired.set_disposition(event_id, disposition)
        } else {
            self.desired.set_disposition(event_id, disposition)
        };
        if changed {
            actions.push(LifecycleAction::DispositionChanged {
                event_id,
                disposition,
            });
        }
        changed
    }

    fn is_current_transition_tag(&self, tag: &TransitionTag<I>) -> bool {
        self.matches_current_transition_id(tag) && tag.lifecycle_epoch == self.epoch
    }

    fn matches_current_transition_id(&self, tag: &TransitionTag<I>) -> bool {
        tag.incarnation == self.incarnation
            && self
                .transition
                .is_some_and(|transition| transition.id == tag.transition)
    }

    fn is_current_ordinary_tag(&self, tag: &WorkTag<I>) -> bool {
        tag.incarnation == self.incarnation
            && tag.lifecycle_epoch == self.epoch
            && tag.transition.is_none()
            && self.transition.is_none()
            && self.admission_open
            && self.state == DeviceLifecycleState::Ready
    }

    fn tag_for(
        &self,
        transition: LifecycleTransitionId,
        epoch: LifecycleEpochId,
    ) -> TransitionTag<I> {
        TransitionTag::new(self.incarnation.clone(), epoch, transition)
    }

    fn current_work_tag(&self) -> WorkTag<I> {
        WorkTag {
            incarnation: self.incarnation.clone(),
            lifecycle_epoch: self.epoch,
            transition: self.transition.map(|transition| transition.id),
        }
    }

    fn force_epoch_bump(&mut self, actions: &mut Vec<LifecycleAction<I>>) {
        if let Some(next) = self.epoch.checked_next() {
            self.epoch = next;
            actions.push(LifecycleAction::EpochAdvanced(next));
        } else {
            actions.push(LifecycleAction::EpochExhausted);
        }
    }
}

fn field_for_kind(kind: LifecycleKind) -> DesiredField {
    match kind {
        LifecycleKind::Shutdown => DesiredField::Shutdown,
        LifecycleKind::DeviceRemoved | LifecycleKind::DeviceAddedOrReplaced => {
            DesiredField::Presence
        }
        LifecycleKind::VTRelease | LifecycleKind::VTAcquire => DesiredField::Seat,
        LifecycleKind::AdministrativeReprobe => DesiredField::AdministrativeReprobe,
        LifecycleKind::IdentityChangingHotplug | LifecycleKind::TopologyRebuild => {
            DesiredField::Topology
        }
        LifecycleKind::DPMS => DesiredField::Dpms,
        LifecycleKind::NormalRecovery => DesiredField::Recovery,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ArbiterInput, CommitProgress, LifecycleAction, LifecycleArbiter, LifecycleCommitOutcome,
        LifecycleReceipt, LifecycleReceiptResult, LifecycleTransitionPhase, field_for_kind,
    };
    use crate::kms::owner::lifecycle::{
        CompletionUnknownRow, DesiredField, DesiredIntent, DeviceLifecycleState, Disposition,
        DpmsTarget, IncidentSeed, LifecycleEpochId, LifecycleEventId, LifecycleKind,
        LifecycleTransitionId, Prerequisite, RecoveryAttemptBudget, RecoveryId,
        RecoveryIdAllocator, RecoveryIncidentState, SeatTarget, TopologyChangeClass, TransitionTag,
        WorkTag,
    };

    type Arbiter = LifecycleArbiter<u32, u32>;
    type Action = LifecycleAction<u32>;

    fn event(raw: u64) -> LifecycleEventId {
        LifecycleEventId::from_raw(raw)
    }

    fn intent(kind: LifecycleKind, generation: u64) -> DesiredIntent {
        match kind {
            LifecycleKind::Shutdown => DesiredIntent::Shutdown,
            LifecycleKind::DeviceRemoved => DesiredIntent::DevicePresence {
                present: false,
                identity_epoch: generation,
            },
            LifecycleKind::VTRelease => DesiredIntent::Seat {
                target: SeatTarget::Released,
                epoch: generation,
            },
            LifecycleKind::DeviceAddedOrReplaced => DesiredIntent::DevicePresence {
                present: true,
                identity_epoch: generation,
            },
            LifecycleKind::VTAcquire => DesiredIntent::Seat {
                target: SeatTarget::Owned,
                epoch: generation,
            },
            LifecycleKind::AdministrativeReprobe => {
                DesiredIntent::AdministrativeReprobe { epoch: generation }
            }
            LifecycleKind::IdentityChangingHotplug => DesiredIntent::Topology {
                discovery_epoch: generation,
                change_class: TopologyChangeClass::IdentityChanging,
            },
            LifecycleKind::TopologyRebuild => DesiredIntent::Topology {
                discovery_epoch: generation,
                change_class: TopologyChangeClass::SameIdentity,
            },
            LifecycleKind::DPMS => DesiredIntent::Dpms {
                level: (generation % 4) as u8,
                epoch: generation,
            },
            LifecycleKind::NormalRecovery => DesiredIntent::NormalRecovery {
                recovery_id: RecoveryId::from_raw(generation.max(1)),
            },
        }
    }

    fn tag(arbiter: &Arbiter) -> TransitionTag<u32> {
        let transition = arbiter.transition().expect("transition is active");
        TransitionTag::new(1, arbiter.epoch(), transition.id)
    }

    fn start(arbiter: &mut Arbiter, kind: LifecycleKind, raw: u64) -> Vec<Action> {
        arbiter.apply(ArbiterInput::LifecycleEvent {
            event_id: event(raw),
            intent: intent(kind, raw),
        })
    }

    fn acknowledge_all(arbiter: &mut Arbiter) -> Vec<Action> {
        let current_tag = tag(arbiter);
        let mut actions = Vec::new();
        for receipt in LifecycleReceipt::ALL {
            actions.extend(arbiter.apply(ArbiterInput::Receipt {
                tag: current_tag,
                receipt,
                result: LifecycleReceiptResult::Succeeded,
            }));
        }
        actions
    }

    fn finish_current(arbiter: &mut Arbiter) -> Vec<Action> {
        let mut actions = acknowledge_all(arbiter);
        let current_tag = tag(arbiter);
        actions.extend(arbiter.apply(ArbiterInput::CommitOutcome {
            tag: current_tag,
            outcome: LifecycleCommitOutcome::Completed {
                dpms_projections_retired: true,
            },
        }));
        actions
    }

    fn seed_prerequisites(arbiter: &mut Arbiter) {
        arbiter.desired.project(
            event(90_001),
            DesiredIntent::DevicePresence {
                present: true,
                identity_epoch: 1,
            },
        );
        arbiter.desired.set_disposition(
            event(90_001),
            Disposition::Applied(LifecycleTransitionId::from_raw(90_001)),
        );
        arbiter.desired.project(
            event(90_002),
            DesiredIntent::Seat {
                target: SeatTarget::Owned,
                epoch: 1,
            },
        );
        arbiter.desired.set_disposition(
            event(90_002),
            Disposition::Applied(LifecycleTransitionId::from_raw(90_002)),
        );
    }

    fn field_intent(field: DesiredField, generation: u64) -> DesiredIntent {
        match field {
            DesiredField::Shutdown => DesiredIntent::Shutdown,
            DesiredField::Presence => intent(LifecycleKind::DeviceAddedOrReplaced, generation),
            DesiredField::Seat => intent(LifecycleKind::VTAcquire, generation),
            DesiredField::AdministrativeReprobe => {
                intent(LifecycleKind::AdministrativeReprobe, generation)
            }
            DesiredField::Topology => intent(LifecycleKind::IdentityChangingHotplug, generation),
            DesiredField::Dpms => intent(LifecycleKind::DPMS, generation),
            DesiredField::Recovery => intent(LifecycleKind::NormalRecovery, generation),
        }
    }

    fn settle_without_driver(arbiter: &mut Arbiter) -> Vec<LifecycleKind> {
        let mut winners = Vec::new();
        if let Some(active) = arbiter.transition {
            let mut ignored = Vec::new();
            arbiter.mark_active_representative_applied(active, &mut ignored);
            arbiter.transition = None;
        }
        arbiter.state = DeviceLifecycleState::Ready;
        arbiter.admission_open = true;
        loop {
            arbiter.converge();
            let Some(active) = arbiter.transition else {
                break;
            };
            winners.push(active.kind);
            let mut ignored = Vec::new();
            arbiter.mark_active_representative_applied(active, &mut ignored);
            arbiter.transition = None;
            arbiter.state = DeviceLifecycleState::Ready;
            arbiter.admission_open = true;
        }
        winners
    }

    #[test]
    fn c0_3a_every_pair_elects_by_precedence() {
        for active_kind in LifecycleKind::ALL {
            for arriving_kind in LifecycleKind::ALL {
                let mut arbiter = Arbiter::new(1);
                seed_prerequisites(&mut arbiter);
                start(&mut arbiter, active_kind, 1);
                let active_id = arbiter.transition().unwrap().id;

                arbiter.apply(ArbiterInput::LifecycleEvent {
                    event_id: event(2),
                    intent: intent(arriving_kind, 2),
                });
                let current = arbiter.transition().expect("one winner remains");
                if arriving_kind.outranks(active_kind) {
                    assert_ne!(
                        current.id, active_id,
                        "{active_kind:?} <- {arriving_kind:?}"
                    );
                    assert_eq!(
                        current.kind, arriving_kind,
                        "{active_kind:?} <- {arriving_kind:?}"
                    );
                } else {
                    assert_eq!(
                        current.id, active_id,
                        "{active_kind:?} <- {arriving_kind:?}"
                    );
                    assert_eq!(
                        current.kind, active_kind,
                        "{active_kind:?} <- {arriving_kind:?}"
                    );
                }
                assert!(arbiter.transition().is_some());
            }
        }
    }

    #[test]
    fn c0_3a_never_two_transitions() {
        for first in LifecycleKind::ALL {
            for second in LifecycleKind::ALL {
                for third in LifecycleKind::ALL {
                    let mut arbiter = Arbiter::new(1);
                    seed_prerequisites(&mut arbiter);
                    start(&mut arbiter, first, 1);
                    let second_actions = arbiter.apply(ArbiterInput::LifecycleEvent {
                        event_id: event(2),
                        intent: intent(second, 2),
                    });
                    assert!(arbiter.transition().is_some(), "{first:?}, {second:?}");
                    assert!(!second_actions.iter().any(|action| matches!(
                        action,
                        LifecycleAction::PhysicalAdvanceAllowed(_)
                    )));
                    let third_actions = arbiter.apply(ArbiterInput::LifecycleEvent {
                        event_id: event(3),
                        intent: intent(third, 3),
                    });
                    assert!(!third_actions.iter().any(|action| matches!(
                        action,
                        LifecycleAction::PhysicalAdvanceAllowed(_)
                    )));
                    assert!(
                        arbiter.transition().is_some(),
                        "{first:?}, {second:?}, {third:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn c0_3a_logical_now_physical_after_receipts() {
        for winner in [
            LifecycleKind::VTRelease,
            LifecycleKind::DeviceRemoved,
            LifecycleKind::Shutdown,
        ] {
            let mut arbiter = Arbiter::new(1);
            seed_prerequisites(&mut arbiter);
            start(&mut arbiter, LifecycleKind::DPMS, 1);
            let old_tag = tag(&arbiter);
            arbiter.apply(ArbiterInput::CommitProgress {
                tag: old_tag,
                progress: CommitProgress::Accepted,
            });
            let actions = start(&mut arbiter, winner, 2);
            let winning_tag = tag(&arbiter);
            match winner {
                LifecycleKind::VTRelease => {
                    assert!(
                        actions
                            .iter()
                            .any(|action| matches!(action, LifecycleAction::ReleaseSeat(_)))
                    );
                }
                LifecycleKind::DeviceRemoved | LifecycleKind::Shutdown => {
                    assert!(
                        actions
                            .iter()
                            .any(|action| matches!(action, LifecycleAction::WithdrawOutputs(_)))
                    );
                    assert!(actions.iter().any(|action| matches!(
                        action,
                        LifecycleAction::TerminalizeProtocolWork(_)
                    )));
                }
                _ => unreachable!(),
            }
            assert!(
                !actions
                    .iter()
                    .any(|action| matches!(action, LifecycleAction::PhysicalAdvanceAllowed(_)))
            );

            let receipts = LifecycleReceipt::ALL;
            for receipt in receipts.into_iter().take(3) {
                let actions = arbiter.apply(ArbiterInput::Receipt {
                    tag: winning_tag,
                    receipt,
                    result: LifecycleReceiptResult::Succeeded,
                });
                assert!(
                    !actions
                        .iter()
                        .any(|action| matches!(action, LifecycleAction::PhysicalAdvanceAllowed(_)))
                );
            }
            let actions = arbiter.apply(ArbiterInput::Receipt {
                tag: winning_tag,
                receipt: LifecycleReceipt::QuarantineTransferred,
                result: LifecycleReceiptResult::Failed,
            });
            assert!(
                actions
                    .iter()
                    .any(|action| matches!(action, LifecycleAction::ReceiptFailed { .. }))
            );
            assert_eq!(
                arbiter.transition().unwrap().phase,
                LifecycleTransitionPhase::PhysicallyFenced
            );
            assert!(
                !actions
                    .iter()
                    .any(|action| matches!(action, LifecycleAction::PhysicalAdvanceAllowed(_)))
            );

            let mut second = Arbiter::new(1);
            seed_prerequisites(&mut second);
            start(&mut second, LifecycleKind::DPMS, 10);
            start(&mut second, LifecycleKind::VTRelease, 11);
            let stale_tag = tag(&second);
            start(&mut second, LifecycleKind::Shutdown, 12);
            let newest_tag = tag(&second);
            let mut retagged_stale = stale_tag;
            retagged_stale.lifecycle_epoch = newest_tag.lifecycle_epoch;
            for receipt in receipts {
                let stale = second.apply(ArbiterInput::Receipt {
                    tag: retagged_stale,
                    receipt,
                    result: LifecycleReceiptResult::Succeeded,
                });
                assert!(
                    stale.iter().any(|action| matches!(
                        action,
                        LifecycleAction::StaleReceiptIgnored { .. }
                    ))
                );
            }
            for receipt in receipts.into_iter().take(3) {
                let actions = second.apply(ArbiterInput::Receipt {
                    tag: newest_tag,
                    receipt,
                    result: LifecycleReceiptResult::Succeeded,
                });
                assert!(
                    !actions
                        .iter()
                        .any(|action| matches!(action, LifecycleAction::PhysicalAdvanceAllowed(_)))
                );
            }
            let actions = second.apply(ArbiterInput::Receipt {
                tag: newest_tag,
                receipt: LifecycleReceipt::QuarantineTransferred,
                result: LifecycleReceiptResult::Succeeded,
            });
            assert!(actions.contains(&LifecycleAction::PhysicalAdvanceAllowed(newest_tag)));

            let mut fenced = Arbiter::new(1);
            seed_prerequisites(&mut fenced);
            start(&mut fenced, LifecycleKind::DPMS, 20);
            start(&mut fenced, LifecycleKind::VTRelease, 21);
            let failed_winner_tag = tag(&fenced);
            for receipt in receipts.into_iter().take(3) {
                fenced.apply(ArbiterInput::Receipt {
                    tag: failed_winner_tag,
                    receipt,
                    result: LifecycleReceiptResult::Succeeded,
                });
            }
            fenced.apply(ArbiterInput::Receipt {
                tag: failed_winner_tag,
                receipt: LifecycleReceipt::QuarantineTransferred,
                result: LifecycleReceiptResult::Failed,
            });
            start(&mut fenced, LifecycleKind::Shutdown, 22);
            let fenced_winner_tag = tag(&fenced);
            let mut latest_actions = Vec::new();
            for receipt in LifecycleReceipt::ALL {
                latest_actions.extend(fenced.apply(ArbiterInput::Receipt {
                    tag: fenced_winner_tag,
                    receipt,
                    result: LifecycleReceiptResult::Succeeded,
                }));
            }
            assert!(
                !latest_actions
                    .iter()
                    .any(|action| matches!(action, LifecycleAction::PhysicalAdvanceAllowed(_)))
            );
            let cleared = fenced.apply(ArbiterInput::FdFamilyBarrierCleared {
                tag: fenced_winner_tag,
            });
            assert!(cleared.contains(&LifecycleAction::PhysicalAdvanceAllowed(fenced_winner_tag)));
        }
    }

    #[test]
    fn c0_3a_submitted_is_never_cancelled_as_never_submitted() {
        let mut arbiter = Arbiter::new(1);
        seed_prerequisites(&mut arbiter);
        start(&mut arbiter, LifecycleKind::DPMS, 1);
        let old_tag = tag(&arbiter);
        arbiter.apply(ArbiterInput::CommitProgress {
            tag: old_tag,
            progress: CommitProgress::Accepted,
        });
        let actions = start(&mut arbiter, LifecycleKind::Shutdown, 2);
        assert!(actions.contains(&LifecycleAction::AwaitTerminalState(old_tag)));
        assert!(!actions.contains(&LifecycleAction::CancelPreSubmitWork(old_tag)));
    }

    #[test]
    fn c0_3a_convergence_selects_the_highest_unsatisfied() {
        for mask in 0..(1usize << DesiredField::ALL.len()) {
            for prerequisite_case in 0..3 {
                let mut arbiter = Arbiter::new(1);
                let mut present_fields = Vec::new();
                for (index, field) in DesiredField::ALL.into_iter().enumerate() {
                    if mask & (1 << index) != 0 {
                        present_fields.push(field);
                        let generation =
                            100 + (prerequisite_case * DesiredField::ALL.len() + index) as u64;
                        let selected_intent = match (field, prerequisite_case) {
                            (DesiredField::Presence, 2) => DesiredIntent::DevicePresence {
                                present: false,
                                identity_epoch: generation,
                            },
                            (DesiredField::Seat, 1) => DesiredIntent::Seat {
                                target: SeatTarget::Released,
                                epoch: generation,
                            },
                            _ => field_intent(field, generation),
                        };
                        arbiter.desired.project(event(generation), selected_intent);
                    }
                }
                let actions = arbiter.converge();
                let expected = arbiter
                    .desired
                    .representatives()
                    .iter()
                    .map(|representative| representative.kind)
                    .filter(|kind| match kind {
                        LifecycleKind::DeviceAddedOrReplaced => {
                            arbiter.desired.seat_target() == Some(SeatTarget::Owned)
                        }
                        LifecycleKind::VTAcquire => arbiter.desired.device_presence() == Some(true),
                        _ => true,
                    })
                    .min_by_key(|kind| kind.precedence());
                assert_eq!(
                    arbiter.transition().map(|transition| transition.kind),
                    expected,
                    "mask {mask:#x}, prerequisite case {prerequisite_case}"
                );
                for field in present_fields {
                    let Some(representative) = arbiter.desired.representative(field) else {
                        continue;
                    };
                    let is_winner = arbiter
                        .transition()
                        .is_some_and(|transition| transition.kind == representative.kind);
                    if !is_winner {
                        let expected_prerequisite = match representative.kind {
                            LifecycleKind::DeviceAddedOrReplaced
                                if arbiter.desired.seat_target() != Some(SeatTarget::Owned) =>
                            {
                                Some(Prerequisite::SeatReleased)
                            }
                            LifecycleKind::VTAcquire
                                if arbiter.desired.device_presence() != Some(true) =>
                            {
                                Some(Prerequisite::DeviceAbsent)
                            }
                            _ => None,
                        };
                        if let Some(prerequisite) = expected_prerequisite {
                            assert_eq!(
                                arbiter.desired.disposition(representative.event_id),
                                Some(Disposition::Deferred(prerequisite))
                            );
                        }
                    }
                }
                if expected.is_none() {
                    assert!(!actions.iter().any(|action| matches!(
                        action,
                        LifecycleAction::PhysicalAdvanceAllowed(_)
                    )));
                }
            }
        }
    }

    #[test]
    fn c0_3a_mixed_arrivals_keep_every_lower_field() {
        for active in LifecycleKind::ALL {
            let lower: Vec<_> = LifecycleKind::ALL
                .into_iter()
                .filter(|kind| kind.precedence() > active.precedence())
                .collect();
            let subset_count = 1usize << lower.len();
            for mask in 0..subset_count {
                let selected: Vec<_> = lower
                    .iter()
                    .enumerate()
                    .filter_map(|(index, kind)| ((mask & (1 << index)) != 0).then_some(*kind))
                    .collect();
                let arrivals = mixed_arrivals(&selected);
                let groups = mixed_field_groups(&arrivals);
                let expected = expected_mixed_ledger(active, &arrivals);
                let full_mix = mask + 1 == subset_count;

                if full_mix {
                    // The full mix gets sorted and reverse field orders, a
                    // rotated order with each field's generations adjacent,
                    // generations split across fields, and 32 deterministic
                    // shuffled field orders. Shuffling fields while
                    // preserving each field's generation chronology checks
                    // that independent updates commute.
                    let sorted = flatten_mixed_groups(&groups, 0..groups.len());
                    let reverse = flatten_mixed_groups(&groups, (0..groups.len()).rev());
                    let adjacent_generations = if groups.is_empty() {
                        Vec::new()
                    } else {
                        flatten_mixed_groups(&groups, (1..groups.len()).chain(std::iter::once(0)))
                    };
                    let split_generations = split_mixed_generations(&groups);
                    let mut orders = vec![sorted, reverse, adjacent_generations, split_generations];
                    orders.extend(seeded_mixed_orders(
                        &groups,
                        32,
                        0x03a1_u64 ^ (u64::from(active.precedence()) << 32),
                    ));
                    let mut checked = Vec::new();
                    for order in orders {
                        if !checked.contains(&order) {
                            assert_mixed_ledger_and_winners(active, &order, &expected);
                            checked.push(order);
                        }
                    }
                } else if arrivals.len() <= 4 {
                    // Exhaust every interleaving of independent fields for
                    // small mixes; generations within a field stay ordered.
                    permute_mixed_groups(&groups, |order| {
                        assert_mixed_ledger_and_winners(active, &order, &expected);
                    });
                } else {
                    let sorted = flatten_mixed_groups(&groups, 0..groups.len());
                    assert_mixed_ledger_and_winners(active, &sorted, &expected);
                }
            }
        }
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    struct MixedArrival {
        kind: LifecycleKind,
        generation: u64,
        event_id: LifecycleEventId,
    }

    #[derive(Debug, Eq, PartialEq)]
    struct ExpectedMixedLedger {
        dispositions: Vec<(LifecycleEventId, Disposition)>,
        representatives: Vec<(LifecycleEventId, DesiredField, LifecycleKind, Disposition)>,
        winners: Vec<LifecycleKind>,
    }

    fn latest_wins_field(field: DesiredField) -> bool {
        matches!(
            field,
            DesiredField::Presence
                | DesiredField::Seat
                | DesiredField::AdministrativeReprobe
                | DesiredField::Topology
                | DesiredField::Dpms
        )
    }

    fn mixed_event_id(kind: LifecycleKind, generation: u64) -> LifecycleEventId {
        event(100 + u64::from(kind.precedence()) * 10 + generation)
    }

    fn mixed_arrivals(selected: &[LifecycleKind]) -> Vec<MixedArrival> {
        let mut arrivals = Vec::new();
        let mut field_generations: Vec<(DesiredField, u64)> = Vec::new();

        for kind in selected.iter().copied() {
            let field = field_for_kind(kind);
            let generation = if latest_wins_field(field) {
                if let Some((_, current)) = field_generations
                    .iter_mut()
                    .find(|(current, _)| *current == field)
                {
                    *current += 1;
                    *current
                } else {
                    field_generations.push((field, 1));
                    1
                }
            } else {
                1
            };
            arrivals.push(MixedArrival {
                kind,
                generation,
                event_id: mixed_event_id(kind, generation),
            });
        }

        // A singleton latest-wins field needs an explicit second generation.
        // If two different lower kinds share a field, those two arrivals are
        // already its first and second generations.
        let singleton_fields: Vec<_> = field_generations
            .iter()
            .filter_map(|(field, count)| (*count == 1).then_some(*field))
            .collect();
        for field in singleton_fields {
            let first = arrivals
                .iter()
                .find(|arrival| field_for_kind(arrival.kind) == field)
                .copied()
                .expect("singleton field has an arrival");
            arrivals.push(MixedArrival {
                kind: first.kind,
                generation: 2,
                event_id: mixed_event_id(first.kind, 2),
            });
        }
        arrivals
    }

    fn mixed_field_groups(arrivals: &[MixedArrival]) -> Vec<Vec<MixedArrival>> {
        let mut groups: Vec<Vec<MixedArrival>> = Vec::new();
        for arrival in arrivals.iter().copied() {
            let field = field_for_kind(arrival.kind);
            if let Some(group) = groups
                .iter_mut()
                .find(|group| field_for_kind(group[0].kind) == field)
            {
                group.push(arrival);
            } else {
                groups.push(vec![arrival]);
            }
        }
        for group in &mut groups {
            group.sort_by_key(|arrival| arrival.generation);
        }
        groups.sort_by_key(|group| {
            group
                .iter()
                .map(|arrival| arrival.kind.precedence())
                .min()
                .unwrap_or(u8::MAX)
        });
        groups
    }

    fn flatten_mixed_groups(
        groups: &[Vec<MixedArrival>],
        order: impl Iterator<Item = usize>,
    ) -> Vec<MixedArrival> {
        let mut arrivals = Vec::new();
        for index in order {
            arrivals.extend(groups[index].iter().copied());
        }
        arrivals
    }

    fn permute_mixed_groups(
        groups: &[Vec<MixedArrival>],
        mut visit: impl FnMut(Vec<MixedArrival>),
    ) {
        fn recurse(
            groups: &[Vec<MixedArrival>],
            used: &mut [bool],
            order: &mut Vec<usize>,
            visit: &mut impl FnMut(Vec<MixedArrival>),
        ) {
            if order.len() == groups.len() {
                visit(flatten_mixed_groups(groups, order.iter().copied()));
                return;
            }
            for index in 0..groups.len() {
                if !used[index] {
                    used[index] = true;
                    order.push(index);
                    recurse(groups, used, order, visit);
                    order.pop();
                    used[index] = false;
                }
            }
        }

        recurse(
            groups,
            &mut vec![false; groups.len()],
            &mut Vec::new(),
            &mut visit,
        );
    }

    fn split_mixed_generations(groups: &[Vec<MixedArrival>]) -> Vec<MixedArrival> {
        let mut order = Vec::new();
        for generation in [1, 2] {
            for group in groups {
                order.extend(
                    group
                        .iter()
                        .filter(|arrival| arrival.generation == generation)
                        .copied(),
                );
            }
        }
        order
    }

    fn seeded_mixed_orders(
        groups: &[Vec<MixedArrival>],
        sample_count: usize,
        mut seed: u64,
    ) -> Vec<Vec<MixedArrival>> {
        let mut orders = Vec::with_capacity(sample_count);
        for _ in 0..sample_count {
            let mut indexes: Vec<_> = (0..groups.len()).collect();
            for index in (1..indexes.len()).rev() {
                // xorshift64* gives a small deterministic shuffle without a
                // dependency or use of the process-global random seed.
                seed ^= seed >> 12;
                seed ^= seed << 25;
                seed ^= seed >> 27;
                let random = seed.wrapping_mul(0x2545_f491_4f6c_dd1d);
                indexes.swap(index, (random % (index as u64 + 1)) as usize);
            }
            orders.push(flatten_mixed_groups(groups, indexes.into_iter()));
        }
        orders
    }

    fn latest_mixed_by_field(arrivals: &[MixedArrival]) -> Vec<(DesiredField, MixedArrival)> {
        let mut latest: Vec<(DesiredField, MixedArrival)> = Vec::new();
        for arrival in arrivals.iter().copied() {
            let field = field_for_kind(arrival.kind);
            if let Some((_, previous)) = latest.iter_mut().find(|(current, _)| *current == field) {
                if arrival.generation > previous.generation {
                    *previous = arrival;
                }
            } else {
                latest.push((field, arrival));
            }
        }
        latest
    }

    fn expected_deferred(
        kind: LifecycleKind,
        device_present: bool,
        seat_target: SeatTarget,
    ) -> Option<Disposition> {
        match kind {
            LifecycleKind::DeviceAddedOrReplaced if seat_target != SeatTarget::Owned => {
                Some(Disposition::Deferred(Prerequisite::SeatReleased))
            }
            LifecycleKind::VTAcquire if !device_present => {
                Some(Disposition::Deferred(Prerequisite::DeviceAbsent))
            }
            LifecycleKind::DPMS if seat_target == SeatTarget::Released => {
                Some(Disposition::Deferred(Prerequisite::SeatReleased))
            }
            _ => None,
        }
    }

    fn expected_mixed_ledger(
        active: LifecycleKind,
        arrivals: &[MixedArrival],
    ) -> ExpectedMixedLedger {
        let latest = latest_mixed_by_field(arrivals);
        let active_field = field_for_kind(active);
        let first_active_replacement = arrivals
            .iter()
            .filter(|arrival| field_for_kind(arrival.kind) == active_field)
            .min_by_key(|arrival| arrival.generation)
            .copied();

        let device_present = latest
            .iter()
            .find(|(field, _)| *field == DesiredField::Presence)
            .map(|(_, arrival)| arrival.kind == LifecycleKind::DeviceAddedOrReplaced)
            .unwrap_or_else(|| match active {
                LifecycleKind::DeviceRemoved => false,
                LifecycleKind::DeviceAddedOrReplaced => true,
                _ => true,
            });
        let seat_target = latest
            .iter()
            .find(|(field, _)| *field == DesiredField::Seat)
            .map(|(_, arrival)| {
                if arrival.kind == LifecycleKind::VTRelease {
                    SeatTarget::Released
                } else {
                    SeatTarget::Owned
                }
            })
            .unwrap_or_else(|| match active {
                LifecycleKind::VTRelease => SeatTarget::Released,
                _ => SeatTarget::Owned,
            });

        let mut winners: Vec<_> = if active == LifecycleKind::Shutdown {
            Vec::new()
        } else {
            latest
                .iter()
                .filter_map(|(_, arrival)| {
                    expected_deferred(arrival.kind, device_present, seat_target)
                        .is_none()
                        .then_some(arrival.kind)
                })
                .collect()
        };
        winners.sort_by_key(|kind| kind.precedence());

        let disposition_for_latest = |kind: LifecycleKind| {
            if active == LifecycleKind::Shutdown {
                Disposition::Invalidated(crate::kms::owner::lifecycle::InvalidationReason::Shutdown)
            } else if let Some(deferred) = expected_deferred(kind, device_present, seat_target) {
                deferred
            } else {
                let winner_index = winners
                    .iter()
                    .position(|winner| *winner == kind)
                    .expect("runnable representative has a winner");
                Disposition::Applied(LifecycleTransitionId::from_raw(2 + winner_index as u64))
            }
        };

        let mut dispositions = vec![(
            event(1),
            first_active_replacement.map_or(
                Disposition::Applied(LifecycleTransitionId::from_raw(1)),
                |arrival| Disposition::SupersededBy(arrival.event_id),
            ),
        )];
        let mut representatives = Vec::new();
        if first_active_replacement.is_none() {
            representatives.push((
                event(1),
                active_field,
                active,
                Disposition::Applied(LifecycleTransitionId::from_raw(1)),
            ));
        }

        for arrival in arrivals.iter().copied() {
            let disposition = if active == LifecycleKind::Shutdown {
                Disposition::Invalidated(crate::kms::owner::lifecycle::InvalidationReason::Shutdown)
            } else if let Some(next) = arrivals
                .iter()
                .filter(|candidate| {
                    field_for_kind(candidate.kind) == field_for_kind(arrival.kind)
                        && candidate.generation > arrival.generation
                })
                .min_by_key(|candidate| candidate.generation)
            {
                Disposition::SupersededBy(next.event_id)
            } else {
                disposition_for_latest(arrival.kind)
            };
            dispositions.push((arrival.event_id, disposition));
            if !matches!(
                disposition,
                Disposition::SupersededBy(_) | Disposition::Invalidated(_)
            ) && active != LifecycleKind::Shutdown
            {
                representatives.push((
                    arrival.event_id,
                    field_for_kind(arrival.kind),
                    arrival.kind,
                    disposition,
                ));
            }
        }
        dispositions.sort_by_key(|(event_id, _)| event_id.get());
        representatives.sort_by_key(|(event_id, _, _, _)| event_id.get());

        ExpectedMixedLedger {
            dispositions,
            representatives,
            winners,
        }
    }

    fn record_mixed_disposition(
        ledger: &mut Vec<(LifecycleEventId, Disposition)>,
        event_id: LifecycleEventId,
        disposition: Disposition,
    ) {
        if let Some((_, current)) = ledger.iter_mut().find(|(current, _)| *current == event_id) {
            *current = disposition;
        } else {
            ledger.push((event_id, disposition));
        }
    }

    fn assert_mixed_ledger_and_winners(
        active: LifecycleKind,
        arrivals: &[MixedArrival],
        expected: &ExpectedMixedLedger,
    ) {
        let mut arbiter = Arbiter::new(1);
        seed_prerequisites(&mut arbiter);
        start(&mut arbiter, active, 1);
        let mut dispositions = Vec::new();
        for arrival in arrivals.iter().copied() {
            let result = arbiter.apply(ArbiterInput::LifecycleEvent {
                event_id: arrival.event_id,
                intent: intent(arrival.kind, arrival.generation),
            });
            for action in result {
                if let LifecycleAction::DispositionChanged {
                    event_id,
                    disposition,
                } = action
                {
                    record_mixed_disposition(&mut dispositions, event_id, disposition);
                }
            }
        }
        let winners = settle_without_driver(&mut arbiter);
        for representative in arbiter.desired.representatives() {
            if (representative.event_id == event(1)
                || arrivals
                    .iter()
                    .any(|arrival| arrival.event_id == representative.event_id))
                && let Some(disposition) = representative.disposition
            {
                record_mixed_disposition(&mut dispositions, representative.event_id, disposition);
            }
        }

        dispositions.sort_by_key(|(event_id, _)| event_id.get());
        assert_eq!(
            dispositions, expected.dispositions,
            "active {active:?}, arrivals {arrivals:?}"
        );

        let mut representatives: Vec<_> = arbiter
            .desired
            .representatives()
            .iter()
            .filter(|representative| {
                representative.event_id == event(1)
                    || arrivals
                        .iter()
                        .any(|arrival| arrival.event_id == representative.event_id)
            })
            .map(|representative| {
                (
                    representative.event_id,
                    representative.field,
                    representative.kind,
                    representative
                        .disposition
                        .expect("every retained mixed representative settles or defers"),
                )
            })
            .collect();
        representatives.sort_by_key(|(event_id, _, _, _)| event_id.get());
        assert_eq!(
            representatives, expected.representatives,
            "active {active:?}, arrivals {arrivals:?}"
        );
        assert_eq!(
            winners, expected.winners,
            "active {active:?}, arrivals {arrivals:?}"
        );
    }

    #[test]
    fn c0_3a_added_and_acquire_converge_in_either_order() {
        for order in [
            [
                LifecycleKind::DeviceAddedOrReplaced,
                LifecycleKind::VTAcquire,
            ],
            [
                LifecycleKind::VTAcquire,
                LifecycleKind::DeviceAddedOrReplaced,
            ],
        ] {
            let mut arbiter = Arbiter::new(1);
            for (index, kind) in order.into_iter().enumerate() {
                arbiter.apply(ArbiterInput::LifecycleEvent {
                    event_id: event(10 + index as u64),
                    intent: intent(kind, 10 + index as u64),
                });
                if index == 0 {
                    assert!(arbiter.transition().is_none());
                    let field = if kind == LifecycleKind::DeviceAddedOrReplaced {
                        DesiredField::Presence
                    } else {
                        DesiredField::Seat
                    };
                    let expected = if kind == LifecycleKind::DeviceAddedOrReplaced {
                        Prerequisite::SeatReleased
                    } else {
                        Prerequisite::DeviceAbsent
                    };
                    assert_eq!(
                        arbiter.desired.representative(field).unwrap().disposition,
                        Some(Disposition::Deferred(expected)),
                    );
                }
            }
            while arbiter.transition().is_some() {
                finish_current(&mut arbiter);
            }
            assert_eq!(arbiter.desired.representatives().len(), 2);
            assert!(
                arbiter
                    .desired
                    .representatives()
                    .iter()
                    .all(|entry| { entry.disposition.is_some_and(Disposition::is_terminal) })
            );
        }
    }

    #[test]
    fn c0_3a_epoch_bumps_once_and_before_invalidation() {
        let mut clean = Arbiter::new(1);
        start(&mut clean, LifecycleKind::DPMS, 1);
        let before = clean.epoch();
        finish_current(&mut clean);
        assert_eq!(clean.epoch(), before, "clean drain remains under its epoch");

        let mut coalesced = Arbiter::new(1);
        start(&mut coalesced, LifecycleKind::DPMS, 10);
        let mut bumps = 0;
        for generation in 11..21 {
            let actions = start(&mut coalesced, LifecycleKind::DPMS, generation);
            bumps += actions
                .iter()
                .filter(|action| matches!(action, LifecycleAction::EpochAdvanced(_)))
                .count();
        }
        assert_eq!(bumps, 1);
        assert_eq!(coalesced.epoch(), LifecycleEpochId::from_raw(2));

        let mut abandoned = Arbiter::new(1);
        start(&mut abandoned, LifecycleKind::DPMS, 30);
        let abandoned_id = abandoned.transition().unwrap().id;
        let actions = start(&mut abandoned, LifecycleKind::Shutdown, 31);
        assert!(matches!(
            actions.first(),
            Some(LifecycleAction::CloseAdmission(_))
        ));
        let bump_index = actions
            .iter()
            .position(|action| matches!(action, LifecycleAction::EpochAdvanced(_)))
            .expect("forced abandonment advances epoch");
        let invalidation_index = actions
            .iter()
            .position(|action| {
                matches!(
                    action,
                    LifecycleAction::DispositionChanged {
                        disposition: Disposition::Invalidated(_),
                        ..
                    }
                )
            })
            .expect("shutdown invalidates old representatives");
        assert!(bump_index < invalidation_index);
        let abandoned_action_index = actions
            .iter()
            .position(|action| {
                matches!(
                    action,
                    LifecycleAction::CancelPreSubmitWork(tag) if tag.transition == abandoned_id
                )
            })
            .expect("pre-submit transition is cancelled after fencing its epoch");
        assert!(bump_index < abandoned_action_index);
    }

    #[test]
    fn c0_3a_ordinary_work_is_tagged_without_a_transition() {
        let mut arbiter = Arbiter::new(1);
        let initial = arbiter.ordinary_work_tag().unwrap();
        assert_eq!(initial, WorkTag::ordinary(1, LifecycleEpochId::first()));
        assert_eq!(initial.transition, None);
        start(&mut arbiter, LifecycleKind::DPMS, 1);
        finish_current(&mut arbiter);
        let ordinary = arbiter.ordinary_work_tag().unwrap();
        assert_eq!(ordinary.lifecycle_epoch, arbiter.epoch());
        assert_eq!(ordinary.transition, None);

        start(&mut arbiter, LifecycleKind::DPMS, 2);
        start(&mut arbiter, LifecycleKind::DPMS, 3);
        assert!(arbiter.epoch() > ordinary.lifecycle_epoch);
        finish_current(&mut arbiter);
        let actions = arbiter.apply(ArbiterInput::OrdinaryReply { tag: ordinary });
        assert!(
            actions
                .iter()
                .any(|action| matches!(action, LifecycleAction::StaleResultIgnored { .. }))
        );
    }

    #[test]
    fn c0_3a_outcomes_map_to_dispositions() {
        let mut completed = Arbiter::new(1);
        completed.add_protocol_output(8);
        start(&mut completed, LifecycleKind::DPMS, 1);
        let current_tag = tag(&completed);
        completed.apply(ArbiterInput::CommitOutcome {
            tag: current_tag,
            outcome: LifecycleCommitOutcome::Completed {
                dpms_projections_retired: true,
            },
        });
        assert_eq!(
            completed.desired.disposition(event(1)),
            Some(Disposition::Applied(current_tag.transition))
        );

        for latched in [Some(9), None] {
            let mut rejected = Arbiter::new(1);
            start(
                &mut rejected,
                LifecycleKind::DPMS,
                20 + u64::from(latched.is_some()),
            );
            let rejected_tag = tag(&rejected);
            rejected.apply(ArbiterInput::CommitOutcome {
                tag: rejected_tag,
                outcome: LifecycleCommitOutcome::Rejected {
                    topology_latched_generation: latched,
                },
            });
            let expected = latched.map_or(
                Disposition::Deferred(Prerequisite::ReadinessClosed),
                |generation| Disposition::Deferred(Prerequisite::TopologyLatched(generation)),
            );
            let representative = rejected.desired.representative(DesiredField::Dpms).unwrap();
            assert_eq!(representative.disposition, Some(expected));
            assert!(!matches!(
                representative.disposition,
                Some(Disposition::Applied(_))
            ));
            if let Some(generation) = latched {
                let actions = rejected.apply(ArbiterInput::LifecycleEvent {
                    event_id: event(30),
                    intent: DesiredIntent::Dpms {
                        level: 0,
                        epoch: 30,
                    },
                });
                assert!(rejected.transition().is_none());
                assert!(
                    !actions
                        .iter()
                        .any(|action| matches!(action, LifecycleAction::PhysicalAdvanceAllowed(_)))
                );
                assert_eq!(
                    rejected
                        .desired
                        .representative(DesiredField::Dpms)
                        .unwrap()
                        .disposition,
                    Some(Disposition::Deferred(Prerequisite::TopologyLatched(
                        generation
                    ))),
                );
            }
        }

        let mut stale = Arbiter::new(1);
        seed_prerequisites(&mut stale);
        start(&mut stale, LifecycleKind::DPMS, 40);
        let stale_tag = tag(&stale);
        start(&mut stale, LifecycleKind::VTRelease, 41);
        let actions = stale.apply(ArbiterInput::CommitOutcome {
            tag: stale_tag,
            outcome: LifecycleCommitOutcome::Completed {
                dpms_projections_retired: true,
            },
        });
        assert!(
            actions
                .iter()
                .any(|action| matches!(action, LifecycleAction::StaleResultIgnored { .. }))
        );
        assert!(!matches!(
            stale.desired.disposition(event(40)),
            Some(Disposition::Applied(_))
        ));

        let mut lost = Arbiter::new(1);
        lost.add_protocol_output(1);
        start(&mut lost, LifecycleKind::DPMS, 50);
        let outcome_actions = lost.apply(ArbiterInput::CompletionUnknown {
            event_id: event(51),
            boundary_recovery_id: None,
        });
        let result = outcome_actions
            .iter()
            .find_map(|action| match action {
                LifecycleAction::CompletionLossTableU { row, outcome } => Some((*row, *outcome)),
                _ => None,
            })
            .unwrap();
        assert_eq!(
            result.0,
            CompletionUnknownRow::DPMS {
                target: DpmsTarget::Off
            }
        );
        assert!(result.1.recovery.new_incident.is_some());
        assert!(outcome_actions.iter().any(|action| matches!(action, LifecycleAction::AllocateRecoveryIncident { event_id: id, .. } if *id == event(51))));
        assert_eq!(lost.state(), DeviceLifecycleState::Poisoned);
        assert_eq!(
            lost.desired.disposition(event(50)),
            Some(Disposition::Deferred(Prerequisite::ReadinessClosed))
        );

        let mut stale_epoch = Arbiter::new(1);
        start(&mut stale_epoch, LifecycleKind::DPMS, 60);
        let old = tag(&stale_epoch);
        let mut old_epoch = old;
        old_epoch.lifecycle_epoch = LifecycleEpochId::first();
        stale_epoch.apply(ArbiterInput::LifecycleEvent {
            event_id: event(61),
            intent: intent(LifecycleKind::DPMS, 61),
        });
        let actions = stale_epoch.apply(ArbiterInput::CommitOutcome {
            tag: old_epoch,
            outcome: LifecycleCommitOutcome::Completed {
                dpms_projections_retired: true,
            },
        });
        assert!(
            actions
                .iter()
                .any(|action| matches!(action, LifecycleAction::StaleResultIgnored { .. }))
        );
        assert!(!matches!(
            stale_epoch.desired.disposition(event(61)),
            Some(Disposition::Applied(_))
        ));
    }

    #[test]
    fn c0_3a_poisoned_dpms_is_logical_only() {
        let mut allocator = RecoveryIdAllocator::new();
        let seed = IncidentSeed::completion_loss(event(1), RecoveryIncidentState::Paused);
        let incident = allocator.allocate_incident(seed).unwrap();
        let recovery_id = incident.id();
        let mut arbiter = Arbiter::new(1);
        arbiter.recovery = Some(incident);
        arbiter
            .desired
            .project(event(1), DesiredIntent::NormalRecovery { recovery_id });
        arbiter.state = DeviceLifecycleState::Poisoned;

        let off = start(&mut arbiter, LifecycleKind::DPMS, 2);
        assert!(arbiter.transition().is_none());
        assert!(
            !off.iter()
                .any(|action| matches!(action, LifecycleAction::PhysicalAdvanceAllowed(_)))
        );
        assert_eq!(
            arbiter.desired.disposition(event(2)),
            Some(Disposition::Deferred(Prerequisite::ReadinessClosed))
        );
        assert_eq!(arbiter.recovery().unwrap().id(), recovery_id);
        assert_eq!(
            arbiter.recovery().unwrap().state(),
            RecoveryIncidentState::Paused
        );

        let on = arbiter.apply(ArbiterInput::LifecycleEvent {
            event_id: event(3),
            intent: DesiredIntent::Dpms { level: 0, epoch: 3 },
        });
        assert!(
            !on.iter()
                .any(|action| matches!(action, LifecycleAction::PhysicalAdvanceAllowed(_)))
        );
        assert!(on.iter().any(|action| matches!(
            action,
            LifecycleAction::DispositionChanged {
                event_id: displaced,
                disposition: Disposition::SupersededBy(newer),
            } if *displaced == event(2) && *newer == event(3)
        )));
        assert_eq!(
            arbiter.desired.disposition(event(3)),
            Some(Disposition::Deferred(Prerequisite::ReadinessClosed))
        );
        assert_eq!(arbiter.recovery().unwrap().id(), recovery_id);
        assert_eq!(
            arbiter.recovery().unwrap().state(),
            RecoveryIncidentState::Active
        );
        assert_eq!(
            arbiter.recovery().unwrap().budget(),
            RecoveryAttemptBudget::Available
        );
        assert!(
            !on.iter()
                .any(|action| matches!(action, LifecycleAction::AllocateRecoveryIncident { .. }))
        );
    }
}
