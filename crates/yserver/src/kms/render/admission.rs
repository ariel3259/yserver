//! Stage 2c-ii's fixture-level admission conductor.
//!
//! Producers are deliberately represented by [`AdmissionSource`] here. The
//! real producer conversion is stage 2c-iii; this module owns the boundary
//! between that source, A1's pure decider, and the managed 2c-i seams.

use std::{
    collections::{BTreeMap, BTreeSet, HashSet, VecDeque},
    sync::Arc,
};

use crate::{
    kms::{
        owner::{
            admission::{
                Admission, AdmissionDecision, AdmissionError, AdmissionToken, Admitted,
                CarriedMaintenance, Confirmed, CrtcId, DirectSuccessor, IntentKey,
                MaintenanceClass, MaintenanceKey, Readiness, ReadinessSnapshot, Reentry,
                ReentryKind, Tier, WaitReason,
            },
            build::CommitDescription,
            clock::{ClockKey, ClockSource, ProbeOutcome, ProbeState},
            device::{DispatchError, FallibleBeginError, OwnerEvent},
            identity::{CommitId, IncarnationId},
            lifecycle::{
                ArbiterInput, ClientModesetId, ClientModesetTag, CommitProgress, LifecycleAction,
                LifecycleCommitOutcome, LifecycleReceipt, LifecycleReceiptResult, TopologyWork,
                TransitionTag,
            },
            record::{FailureCause, RefusalCause, TerminalState},
        },
        render::{
            backend::{
                ClientModesetUnflipHold, DirectEligibility, KmsBackend, PreparedDirectDispatch,
                effective_refresh_matches,
            },
            client_modeset::{
                ClientModesetDescriptionInput, ClientModesetObjects, ClientModesetOperation,
                ClientModesetPropertyIds, OwnedModeBlob, PreparedClientModesetDescription,
                PreparedClientModesetSet, StagedDpmsProjection, build_client_modeset_description,
                stage_dpms_projection,
            },
            platform::CrtcKey,
            resources::{
                CommitResources, GroupMember, ResourceError, register_commit_dependencies,
            },
            scene::PreparedComposedLocation,
        },
    },
    platform::drm::DrmDeviceKey,
};

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum AdmissionOutcome {
    Inert,
    SlotBusy,
    NothingAdmissible,
    Dispatched(Confirmed),
    BeginRefused,
    SendRefused(RefusalCause),
    TransportClosed,
    PreparationRefused,
    Unsupported(Tier),
}

enum DispatchFailureRoute {
    Primary {
        backend_composed: bool,
    },
    Direct {
        retirement: Option<crate::kms::render::resources::RoleReservation>,
    },
    Unflip {
        members: Vec<GroupMember>,
    },
}

#[derive(Clone, Copy, Debug)]
enum DispatchFailureKind {
    Ledger,
    Cleanup,
    Refused,
}

enum DispatchFailureResources {
    Ledger {
        old: Vec<CommitResources>,
        new: Vec<CommitResources>,
    },
    Cleanup {
        old: Vec<CommitResources>,
        new: Vec<CommitResources>,
    },
    Refused {
        new: Vec<CommitResources>,
    },
}

impl DispatchFailureResources {
    fn split(
        self,
    ) -> (
        DispatchFailureKind,
        Vec<CommitResources>,
        Vec<CommitResources>,
    ) {
        match self {
            Self::Ledger { old, new } => (DispatchFailureKind::Ledger, old, new),
            Self::Cleanup { old, new } => (DispatchFailureKind::Cleanup, old, new),
            Self::Refused { new } => (DispatchFailureKind::Refused, Vec::new(), new),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DispatchFailureOutcome {
    BeginRefused,
    TransportClosed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct DispatchFailureAction {
    outcome: DispatchFailureOutcome,
    close_gate: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ResourceRestore {
    Succeeded,
    Failed,
    Missing,
    Extra,
    NotAttempted,
}

fn dispatch_failure_policy(
    route: DispatchFailureRouteKind,
    kind: DispatchFailureKind,
    restore: ResourceRestore,
) -> DispatchFailureAction {
    match (route, kind, restore) {
        (
            DispatchFailureRouteKind::Primary,
            DispatchFailureKind::Ledger,
            ResourceRestore::Succeeded,
        )
        | (
            DispatchFailureRouteKind::Direct,
            DispatchFailureKind::Ledger,
            ResourceRestore::Succeeded,
        )
        | (
            DispatchFailureRouteKind::Primary,
            DispatchFailureKind::Refused,
            ResourceRestore::NotAttempted,
        )
        | (
            DispatchFailureRouteKind::Direct,
            DispatchFailureKind::Refused,
            ResourceRestore::Succeeded | ResourceRestore::Failed,
        ) => DispatchFailureAction {
            outcome: DispatchFailureOutcome::BeginRefused,
            close_gate: false,
        },
        (DispatchFailureRouteKind::Unflip, _, ResourceRestore::Succeeded) => {
            DispatchFailureAction {
                outcome: DispatchFailureOutcome::TransportClosed,
                close_gate: true,
            }
        }
        _ => DispatchFailureAction {
            outcome: DispatchFailureOutcome::TransportClosed,
            close_gate: true,
        },
    }
}

#[derive(Clone, Copy, Debug)]
enum DispatchFailureRouteKind {
    Primary,
    Direct,
    Unflip,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OwnerRefusal {
    SlotOccupied,
    ClockNotReady,
    ReadinessClosed,
    NotYetSupported(ClientModesetUnsupportedFeature),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ClientModesetUnsupportedFeature {
    PositionOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PreparationStage {
    Discovery,
    Mode,
    Route,
    Allocation,
    SceneState,
    #[allow(dead_code, reason = "the prepared unflip path is introduced by 3b-i-2")]
    Unflip,
    TestOnly {
        errno: i32,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ClientModesetFailure {
    Preparation(PreparationStage),
    KernelRejected { errno: i32 },
    OwnerRefused(OwnerRefusal),
    Superseded(crate::kms::owner::lifecycle::LifecycleKind),
    CompletionUnknown,
    Stale,
    Latched,
    SeatReleased,
}

impl std::fmt::Display for ClientModesetFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{self:?}")
    }
}

impl std::error::Error for ClientModesetFailure {}

#[derive(Debug, Clone)]
pub(crate) struct ClientModesetDiagnostic {
    pub(crate) device: DrmDeviceKey,
    pub(crate) modeset: ClientModesetId,
    pub(crate) output_id: u32,
    pub(crate) output: String,
    pub(crate) requested_mode: Option<yserver_core::backend::ModeSpec>,
    pub(crate) x: i32,
    pub(crate) y: i32,
}

#[derive(Debug)]
pub(crate) struct ClientModesetError {
    pub(crate) diagnostic: ClientModesetDiagnostic,
    pub(crate) failure: ClientModesetFailure,
}

impl std::fmt::Display for ClientModesetError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let diagnostic = &self.diagnostic;
        write!(
            formatter,
            "output {} (id {}) mode {:?} position ({}, {}) device {} modeset {} failed: {}",
            diagnostic.output,
            diagnostic.output_id,
            diagnostic.requested_mode,
            diagnostic.x,
            diagnostic.y,
            diagnostic.device,
            diagnostic.modeset.get(),
            self.failure,
        )
    }
}

impl std::error::Error for ClientModesetError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.failure)
    }
}

impl ClientModesetError {
    pub(crate) fn into_io_error(self) -> std::io::Error {
        std::io::Error::other(self)
    }

    pub(crate) fn diagnostic_for_slot(
        device: DrmDeviceKey,
        slot: &ClientModesetSlot,
    ) -> ClientModesetDiagnostic {
        ClientModesetDiagnostic {
            device,
            modeset: slot.tag.modeset,
            output_id: slot.output_id,
            output: slot.connector.clone(),
            requested_mode: slot.mode,
            x: slot.x,
            y: slot.y,
        }
    }
}

fn client_modeset_error_for_slot(
    device: DrmDeviceKey,
    slot: &ClientModesetSlot,
    result: std::io::Result<bool>,
) -> std::io::Result<bool> {
    result.map_err(|error| {
        let failure = client_modeset_failure_from_error(&error).unwrap_or(
            ClientModesetFailure::OwnerRefused(OwnerRefusal::ReadinessClosed),
        );
        ClientModesetError {
            diagnostic: ClientModesetError::diagnostic_for_slot(device, slot),
            failure,
        }
        .into_io_error()
    })
}

fn client_modeset_failure_from_error(error: &std::io::Error) -> Option<ClientModesetFailure> {
    error.get_ref().and_then(|source| {
        source
            .downcast_ref::<ClientModesetError>()
            .map(|error| error.failure)
            .or_else(|| source.downcast_ref::<ClientModesetFailure>().copied())
    })
}

fn test_only_errno_keeps_readiness(errno: i32) -> bool {
    errno == libc::EINVAL || errno == libc::ERANGE || errno == libc::ENOSPC
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ClientModesetLatch {
    pub(crate) topology_generation: u64,
    pub(crate) output_id: u32,
    pub(crate) connector: String,
    pub(crate) mode: Option<yserver_core::backend::ModeSpec>,
    pub(crate) x: i32,
    pub(crate) y: i32,
}

impl ClientModesetLatch {
    fn matches_request(
        &self,
        topology_generation: u64,
        output_id: u32,
        connector: &str,
        mode: Option<yserver_core::backend::ModeSpec>,
        x: i32,
        y: i32,
    ) -> bool {
        self.topology_generation == topology_generation
            && self.output_id == output_id
            && self.connector == connector
            && self.mode == mode
            && self.x == x
            && self.y == y
    }
}

#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AdmissionTraceStep {
    Consumed(CommitId),
    Enqueued {
        completions: Vec<u64>,
        skips: Vec<u64>,
    },
    Decided,
    Dispatched(CommitId),
}

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AdmissionPreparationHook {
    CloseAdmission,
    OccupyOrdinaryRetirement,
}

fn decision_requires_unsupported(decision: &AdmissionDecision) -> bool {
    matches!(decision.admitted, Admitted::CursorRecovery { .. })
}

/// Until Task 5 installs the production producer bridge, lifecycle topology
/// remains dispatchable while every ordinary admission source waits.
struct LifecycleOnlyAdmissionSource;

impl AdmissionSource for LifecycleOnlyAdmissionSource {
    fn producer_readiness(&self, _key: IntentKey) -> Readiness {
        Readiness::Waiting(WaitReason::SourceWaits)
    }

    fn describe(&mut self, _decision: &AdmissionDecision) -> CommitDescription {
        unreachable!("the lifecycle-only source admits topology work only")
    }

    fn maintenance_readiness(&self, _key: MaintenanceKey, _generation: u64) -> Readiness {
        Readiness::Waiting(WaitReason::SourceWaits)
    }

    fn compatible(&self, _key: MaintenanceKey, _generation: u64, _primary: IntentKey) -> bool {
        false
    }

    fn homogeneous_group(&self) -> BTreeSet<CrtcId> {
        BTreeSet::new()
    }

    fn cursor_recovery_ready(&self, _crtc: CrtcId) -> bool {
        false
    }

    fn composed_resources(&mut self, _crtc: CrtcId, _generation: u64) -> Vec<CommitResources> {
        Vec::new()
    }

    fn restore_composed_resources(&mut self, _resources: Vec<CommitResources>) {}

    fn direct_eligible(&self, _source_generation: u64, _eligibility: DirectEligibility) -> bool {
        false
    }
}

enum LifecycleDriverWork {
    Actions {
        requester: Option<TransitionTag<IncarnationId>>,
        actions: Vec<LifecycleAction<IncarnationId>>,
    },
    Input(ArbiterInput<IncarnationId>),
    ValidationResolved {
        commit: CommitId,
        outcome: crate::kms::owner::device::ValidationOutcome,
    },
    TopologyTerminal {
        commit: CommitId,
        tag: TransitionTag<IncarnationId>,
        terminal: TerminalState,
    },
    ClientModesetTerminal {
        commit: CommitId,
        tag: ClientModesetTag<IncarnationId>,
        terminal: TerminalState,
    },
}

enum LifecycleClockReadiness {
    Ready(BTreeMap<u32, ClockKey>),
    Waiting,
    Failed {
        crtc: u32,
        key: ClockKey,
        outcome: ProbeOutcome,
    },
    Missing(u32),
    Inconsistent {
        crtc: u32,
        key: ClockKey,
        detail: &'static str,
    },
}

struct PendingTopologyValidation {
    tag: TransitionTag<IncarnationId>,
    decision: AdmissionDecision,
    token: Option<AdmissionToken>,
    description: CommitDescription,
    dpms_active: bool,
    sent: bool,
    cancelled: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ClientModesetPhase {
    Queued,
    Validating,
    Dispatched,
}

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ClientModesetPromotionStep {
    KmsState,
    Projection,
    Scene,
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) struct ClientModesetSlot {
    pub(crate) tag: ClientModesetTag<IncarnationId>,
    pub(crate) token: yserver_core::backend::CrtcConfigToken,
    pub(crate) output_id: u32,
    pub(crate) connector: String,
    pub(crate) mode: Option<yserver_core::backend::ModeSpec>,
    pub(crate) x: i32,
    pub(crate) y: i32,
    pub(crate) phase: ClientModesetPhase,
    pub(crate) prepared: Option<PreparedClientModesetDescription>,
}

struct PendingClientModesetValidation {
    tag: ClientModesetTag<IncarnationId>,
    decision: AdmissionDecision,
    token: Option<AdmissionToken>,
    description: CommitDescription,
    staged_projection: Option<StagedDpmsProjection>,
    sent: bool,
    cancelled: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct OwnerCrtcPowerChange {
    pub(crate) incarnation: IncarnationId,
    pub(crate) crtc: u32,
    pub(crate) new_active: bool,
    pub(crate) expected_completion: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InstalledCrtcPower {
    Unknown {
        incarnation: IncarnationId,
    },
    Active {
        incarnation: IncarnationId,
    },
    InactiveUnproven {
        incarnation: IncarnationId,
    },
    InactiveProven {
        incarnation: IncarnationId,
        off_commit: CommitId,
    },
}

#[derive(Clone, Copy)]
pub(crate) enum LifecycleOwnerMilestone {
    Accepted(CommitId),
    Terminal(CommitId, TerminalState),
}

/// Per-device run-to-completion effect queue. The coordinator owns the
/// authoritative transition; this object only owns queued effects and the
/// admission token held across TEST_ONLY validation.
pub(crate) struct LifecycleDriver {
    queue: VecDeque<LifecycleDriverWork>,
    pub(crate) draining: bool,
    routing_batch_depth: usize,
    pending_topology_validations: BTreeMap<CommitId, PendingTopologyValidation>,
    pending_client_modeset_validations: BTreeMap<CommitId, PendingClientModesetValidation>,
    topology_commits: BTreeMap<CommitId, TransitionTag<IncarnationId>>,
    client_modeset_commits: BTreeMap<CommitId, ClientModesetTag<IncarnationId>>,
    topology_dpms_active: BTreeMap<CommitId, bool>,
    owner_commit_power_changes: BTreeMap<CommitId, Vec<OwnerCrtcPowerChange>>,
    installed_crtc_power: BTreeMap<u32, InstalledCrtcPower>,
    kms_displacements: BTreeMap<CommitId, Vec<crate::kms::render::resources::KmsReleaseObligation>>,
    pub(crate) client_modeset: Option<ClientModesetSlot>,
    pub(crate) client_modeset_latch: Option<ClientModesetLatch>,
    pub(crate) client_modeset_foreign_busy: bool,
    next_client_modeset_id: Option<u64>,
    #[cfg(test)]
    pub(crate) hook: Option<LifecycleTopologyTestHook>,
    #[cfg(test)]
    deferred_receipts: VecDeque<ArbiterInput<IncarnationId>>,
    #[cfg(test)]
    pub(crate) drain_entries: usize,
    #[cfg(test)]
    pub(crate) queued_while_draining: usize,
    #[cfg(test)]
    pub(crate) applied_inputs: usize,
    #[cfg(test)]
    pub(crate) stale_receipts: usize,
    #[cfg(test)]
    validation_sends: Vec<TransitionTag<IncarnationId>>,
    #[cfg(test)]
    live_sends: Vec<TransitionTag<IncarnationId>>,
    #[cfg(test)]
    client_validation_sends: Vec<ClientModesetTag<IncarnationId>>,
    #[cfg(test)]
    client_live_sends: Vec<ClientModesetTag<IncarnationId>>,
    #[cfg(test)]
    stale_before_test_only: usize,
    #[cfg(test)]
    stale_before_live_dispatch: usize,
    #[cfg(test)]
    stale_results: usize,
    #[cfg(test)]
    pub(crate) client_modeset_promotion_steps: Vec<ClientModesetPromotionStep>,
}

impl LifecycleDriver {
    pub(crate) fn new() -> Self {
        Self {
            queue: VecDeque::new(),
            draining: false,
            routing_batch_depth: 0,
            pending_topology_validations: BTreeMap::new(),
            pending_client_modeset_validations: BTreeMap::new(),
            topology_commits: BTreeMap::new(),
            client_modeset_commits: BTreeMap::new(),
            topology_dpms_active: BTreeMap::new(),
            owner_commit_power_changes: BTreeMap::new(),
            installed_crtc_power: BTreeMap::new(),
            kms_displacements: BTreeMap::new(),
            client_modeset: None,
            client_modeset_latch: None,
            client_modeset_foreign_busy: false,
            next_client_modeset_id: Some(1),
            #[cfg(test)]
            hook: None,
            #[cfg(test)]
            deferred_receipts: VecDeque::new(),
            #[cfg(test)]
            drain_entries: 0,
            #[cfg(test)]
            queued_while_draining: 0,
            #[cfg(test)]
            applied_inputs: 0,
            #[cfg(test)]
            stale_receipts: 0,
            #[cfg(test)]
            validation_sends: Vec::new(),
            #[cfg(test)]
            live_sends: Vec::new(),
            #[cfg(test)]
            client_validation_sends: Vec::new(),
            #[cfg(test)]
            client_live_sends: Vec::new(),
            #[cfg(test)]
            stale_before_test_only: 0,
            #[cfg(test)]
            stale_before_live_dispatch: 0,
            #[cfg(test)]
            stale_results: 0,
            #[cfg(test)]
            client_modeset_promotion_steps: Vec::new(),
        }
    }

    #[cfg(test)]
    pub(crate) fn kms_displacements_for_tests(
        &self,
        commit: CommitId,
    ) -> Vec<crate::kms::render::resources::KmsReleaseObligation> {
        self.kms_displacements
            .get(&commit)
            .cloned()
            .unwrap_or_default()
    }

    #[cfg(test)]
    pub(crate) fn installed_crtc_power_for_tests(&self, crtc: u32) -> Option<InstalledCrtcPower> {
        self.installed_crtc_power.get(&crtc).copied()
    }

    #[cfg(test)]
    pub(crate) fn make_crtc_power_unproven_for_tests(
        &mut self,
        crtc: u32,
        incarnation: IncarnationId,
    ) {
        self.installed_crtc_power
            .insert(crtc, InstalledCrtcPower::Unknown { incarnation });
    }

    fn enqueue(&mut self, work: LifecycleDriverWork) {
        #[cfg(test)]
        if self.draining {
            self.queued_while_draining = self.queued_while_draining.saturating_add(1);
        }
        self.queue.push_back(work);
    }

    fn pop(&mut self) -> Option<LifecycleDriverWork> {
        self.queue.pop_front()
    }

    fn defer_receipt(&mut self, input: ArbiterInput<IncarnationId>) {
        #[cfg(test)]
        self.deferred_receipts.push_back(input);
        self.enqueue(LifecycleDriverWork::Input(input));
    }

    #[cfg(test)]
    pub(crate) fn pending_topology_tags(&self) -> Vec<TransitionTag<IncarnationId>> {
        self.pending_topology_validations
            .values()
            .map(|pending| pending.tag)
            .collect()
    }

    #[cfg(test)]
    pub(crate) fn pending_topology_validation_commits_for_tests(&self) -> Vec<CommitId> {
        self.pending_topology_validations.keys().copied().collect()
    }

    #[cfg(test)]
    pub(crate) fn pending_topology_descriptions_for_tests(&self) -> Vec<&CommitDescription> {
        self.pending_topology_validations
            .values()
            .map(|pending| &pending.description)
            .collect()
    }

    pub(crate) fn has_inflight_topology(&self) -> bool {
        !self.pending_topology_validations.is_empty()
            || !self.pending_client_modeset_validations.is_empty()
            || !self.topology_commits.is_empty()
            || !self.client_modeset_commits.is_empty()
    }

    #[cfg(test)]
    pub(crate) fn track_topology_commit_for_tests(
        &mut self,
        commit: CommitId,
        tag: TransitionTag<IncarnationId>,
    ) {
        self.topology_commits.insert(commit, tag);
    }

    #[cfg(test)]
    pub(crate) fn recorded_receipts(
        &self,
    ) -> Vec<(
        TransitionTag<IncarnationId>,
        LifecycleReceipt,
        LifecycleReceiptResult,
    )> {
        self.deferred_receipts
            .iter()
            .filter_map(|input| match input {
                ArbiterInput::Receipt {
                    tag,
                    receipt,
                    result,
                } => Some((*tag, *receipt, *result)),
                _ => None,
            })
            .collect()
    }

    #[cfg(test)]
    pub(crate) fn topology_test_stats(
        &self,
    ) -> (
        Vec<TransitionTag<IncarnationId>>,
        Vec<TransitionTag<IncarnationId>>,
        usize,
        usize,
        usize,
    ) {
        (
            self.validation_sends.clone(),
            self.live_sends.clone(),
            self.stale_before_test_only,
            self.stale_before_live_dispatch,
            self.stale_results,
        )
    }

    #[cfg(test)]
    pub(crate) fn client_modeset_test_stats(
        &self,
    ) -> (
        Vec<ClientModesetTag<IncarnationId>>,
        Vec<ClientModesetTag<IncarnationId>>,
    ) {
        (
            self.client_validation_sends.clone(),
            self.client_live_sends.clone(),
        )
    }

    #[cfg(test)]
    pub(crate) fn pending_client_modeset_description_for_tests(
        &self,
    ) -> Option<&CommitDescription> {
        self.pending_client_modeset_validations
            .values()
            .next()
            .map(|pending| &pending.description)
    }

    #[cfg(test)]
    pub(crate) fn pending_client_modeset_projection_for_tests(
        &self,
    ) -> Option<StagedDpmsProjection> {
        self.pending_client_modeset_validations
            .values()
            .next()
            .and_then(|pending| pending.staged_projection.clone())
    }

    #[cfg(test)]
    pub(crate) fn run_to_completion_test_stats(&self) -> (usize, usize, usize, usize) {
        (
            self.drain_entries,
            self.queued_while_draining,
            self.applied_inputs,
            self.stale_receipts,
        )
    }

    pub(crate) fn allocate_client_modeset_id(&mut self) -> Option<ClientModesetId> {
        let raw = self.next_client_modeset_id?;
        self.next_client_modeset_id = raw.checked_add(1);
        Some(ClientModesetId::from_raw(raw))
    }
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LifecycleTopologyTestHook {
    SupersedeBeforeTestOnly,
    SupersedeBeforeLiveDispatch,
    ReapExecutorBeforeLiveDispatch,
}

fn decision_primary(decision: &AdmissionDecision) -> Option<&Admitted> {
    match &decision.admitted {
        Admitted::Maintenance { .. } => decision.combined_primary.as_ref(),
        admitted => Some(admitted),
    }
}

fn composed_intents(decision: &AdmissionDecision) -> Vec<(CrtcId, u64)> {
    match decision_primary(decision) {
        Some(Admitted::Composed { crtc, generation }) => vec![(*crtc, *generation)],
        Some(Admitted::Bundle { members }) => members
            .iter()
            .filter_map(|member| match member {
                Admitted::Composed { crtc, generation } => Some((*crtc, *generation)),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

#[derive(Debug, Clone, Copy)]
struct ComposedResourceSpec {
    location: PreparedComposedLocation,
    generation: u64,
    member: GroupMember,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MaintenancePayload {
    pub(crate) generation: u64,
    pub(crate) data: Arc<[u8]>,
}

#[allow(dead_code)]
#[derive(Debug, Default)]
pub(crate) struct MaintenanceStore {
    pub(crate) desired: BTreeMap<MaintenanceKey, MaintenancePayload>,
    pub(crate) submitted: BTreeMap<MaintenanceKey, MaintenancePayload>,
    pub(crate) current: BTreeMap<MaintenanceKey, MaintenancePayload>,
    pub(crate) dormant: BTreeMap<MaintenanceKey, MaintenancePayload>,
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub(crate) struct AdmissionReceipt {
    pub(crate) commit: CommitId,
    pub(crate) carried: Vec<CarriedMaintenance>,
    pub(crate) maintenance_only: bool,
}

/// What 2c-ii cannot observe because producers are converted in 2c-iii.
#[allow(dead_code)]
pub(crate) trait AdmissionSource {
    /// Producer-side readiness: for a composed intent, a reusable buffer and
    /// finished producer waits; for a direct one, its pre-submit source waits.
    fn producer_readiness(&self, key: IntentKey) -> Readiness;
    /// The atomic description of an admitted primary.
    fn describe(&mut self, decision: &AdmissionDecision) -> CommitDescription;
    /// Readiness of a desired maintenance payload supplied by the source.
    fn maintenance_readiness(&self, key: MaintenanceKey, generation: u64) -> Readiness;
    /// Whether a maintenance payload can be carried by the primary intent.
    fn compatible(&self, key: MaintenanceKey, generation: u64, primary: IntentKey) -> bool;
    /// CRTCs whose primary requests may be combined into one commit.
    fn homogeneous_group(&self) -> BTreeSet<CrtcId>;
    /// Whether software-cursor recovery can be admitted for this CRTC.
    fn cursor_recovery_ready(&self, crtc: CrtcId) -> bool;
    /// A composed admission's new-state resources, moved into the ledger.
    fn composed_resources(&mut self, crtc: CrtcId, generation: u64) -> Vec<CommitResources>;
    /// Return resources to the composed intent when owner admission cannot
    /// construct a ledger. The source owns the returned intent-side storage.
    fn restore_composed_resources(&mut self, resources: Vec<CommitResources>);
    /// Whether the queued direct successor passes the production eligibility
    /// result supplied by the backend.
    fn direct_eligible(&self, source_generation: u64, eligibility: DirectEligibility) -> bool;
}

#[allow(dead_code)]
pub(crate) struct AdmissionConductor {
    pub(crate) admission: Admission,
    pub(crate) source: Box<dyn AdmissionSource>,
    /// The converted composed half is served by `KmsBackend` and `SceneCompositor`.
    /// The injected source remains authoritative for direct and maintenance
    /// answers until their later conversion tasks.
    pub(crate) backend_composed: bool,
    pub(crate) layout_generation: u64,
    pub(crate) next_direct_source_generation: u64,
    pub(crate) composed: BTreeMap<CrtcId, u64>,
    pub(crate) maintenance: MaintenanceStore,
    pub(crate) receipts: BTreeMap<CommitId, AdmissionReceipt>,
    pub(crate) recovery_stopped: bool,
    /// Lifecycle work may run while ordinary admission is closed. The
    /// topology tier remains dispatchable; no ordinary tier can pass it.
    pub(crate) lifecycle_admission_closed: bool,
    pub(crate) gamma_failures: BTreeSet<CrtcId>,
    #[cfg(test)]
    pub(crate) prepare_hook: Option<AdmissionPreparationHook>,
    #[cfg(test)]
    pub(crate) force_lock_mismatch: bool,
    #[cfg(test)]
    pub(crate) trace: Vec<AdmissionTraceStep>,
}

#[allow(dead_code)]
impl AdmissionConductor {
    pub(crate) fn new(source: Box<dyn AdmissionSource>) -> Self {
        Self {
            admission: Admission::new(),
            source,
            // Task 5: composed readiness, description, resources and
            // topology come from the live backend/scene path by default.
            backend_composed: true,
            layout_generation: 0,
            next_direct_source_generation: 1,
            composed: BTreeMap::new(),
            maintenance: MaintenanceStore::default(),
            receipts: BTreeMap::new(),
            recovery_stopped: false,
            lifecycle_admission_closed: false,
            gamma_failures: BTreeSet::new(),
            #[cfg(test)]
            prepare_hook: None,
            #[cfg(test)]
            force_lock_mismatch: false,
            #[cfg(test)]
            trace: Vec::new(),
        }
    }

    #[cfg(test)]
    pub(crate) fn new_with_composed_backend(
        source: Box<dyn AdmissionSource>,
        backend_composed: bool,
    ) -> Self {
        Self {
            admission: Admission::new(),
            source,
            backend_composed,
            layout_generation: 0,
            next_direct_source_generation: 1,
            composed: BTreeMap::new(),
            maintenance: MaintenanceStore::default(),
            receipts: BTreeMap::new(),
            recovery_stopped: false,
            lifecycle_admission_closed: false,
            gamma_failures: BTreeSet::new(),
            #[cfg(test)]
            prepare_hook: None,
            #[cfg(test)]
            force_lock_mismatch: false,
            #[cfg(test)]
            trace: Vec::new(),
        }
    }
}

#[allow(dead_code)]
impl KmsBackend {
    pub(crate) fn admission_reserve_client_modeset_id(
        &mut self,
        device: DrmDeviceKey,
    ) -> std::io::Result<ClientModesetId> {
        self.lifecycle_register_owner_device(device)
            .map_err(|error| {
                std::io::Error::other(format!("Owner lifecycle unavailable: {error:?}"))
            })?;
        self.lifecycle_drivers
            .get_mut(&device)
            .expect("Owner lifecycle registration installs a driver")
            .allocate_client_modeset_id()
            .ok_or_else(|| std::io::Error::other("Owner client modeset identity exhausted"))
    }

    pub(crate) fn admission_start_client_modeset(
        &mut self,
        device: DrmDeviceKey,
        modeset: ClientModesetId,
        token: yserver_core::backend::CrtcConfigToken,
        output_id: u32,
        connector: String,
        mode: Option<yserver_core::backend::ModeSpec>,
        x: i32,
        y: i32,
    ) -> std::io::Result<()> {
        let diagnostic = ClientModesetDiagnostic {
            device,
            modeset,
            output_id,
            output: connector.clone(),
            requested_mode: mode,
            x,
            y,
        };
        let refused = |failure| {
            ClientModesetError {
                diagnostic: diagnostic.clone(),
                failure,
            }
            .into_io_error()
        };
        self.lifecycle_register_owner_device(device)
            .map_err(|_error| {
                refused(ClientModesetFailure::OwnerRefused(
                    OwnerRefusal::ReadinessClosed,
                ))
            })?;
        let (incarnation, lifecycle_epoch, topology_generation) = self
            .platform
            .owner_ref(device)
            .map(|owner| {
                (
                    owner.incarnation(),
                    owner.lifecycle_epoch(),
                    owner.topology_generation(),
                )
            })
            .ok_or_else(|| {
                refused(ClientModesetFailure::OwnerRefused(
                    OwnerRefusal::ReadinessClosed,
                ))
            })?;
        if let Some(driver) = self.lifecycle_drivers.get_mut(&device)
            && driver
                .client_modeset_latch
                .as_ref()
                .is_some_and(|latch| latch.topology_generation != topology_generation)
        {
            driver.client_modeset_latch = None;
        }
        let state = self
            .lifecycle_coordinator
            .device(&device)
            .map(|arbiter| arbiter.state());
        if state != Some(crate::kms::owner::lifecycle::DeviceLifecycleState::Ready) {
            return Err(refused(ClientModesetFailure::OwnerRefused(
                OwnerRefusal::ReadinessClosed,
            )));
        }
        if self
            .lifecycle_drivers
            .get(&device)
            .and_then(|driver| driver.client_modeset_latch.as_ref())
            .is_some_and(|latch| {
                latch.matches_request(topology_generation, output_id, &connector, mode, x, y)
            })
        {
            return Err(refused(ClientModesetFailure::Latched));
        }
        if self
            .lifecycle_drivers
            .get(&device)
            .is_some_and(|driver| driver.client_modeset.is_some())
        {
            return Err(refused(ClientModesetFailure::OwnerRefused(
                OwnerRefusal::SlotOccupied,
            )));
        }
        let tag = ClientModesetTag {
            incarnation,
            lifecycle_epoch,
            topology_generation,
            modeset,
        };
        let driver = self
            .lifecycle_drivers
            .get_mut(&device)
            .expect("Owner lifecycle registration installs a driver");
        driver.client_modeset = Some(ClientModesetSlot {
            tag,
            token,
            output_id,
            connector,
            mode,
            x,
            y,
            phase: ClientModesetPhase::Queued,
            prepared: None,
        });
        if let Some(conductor) = self.admission_conductors.get_mut(&device)
            && conductor.admission.topology().is_none()
        {
            let _ = conductor
                .admission
                .request_topology(TopologyWork::ClientModeset(tag));
        }
        self.admission_wake(device, false);
        Ok(())
    }

    /// Retire an Owner client modeset that has not crossed the dispatch
    /// boundary. A dispatched token stays live even after its lifecycle slot
    /// resolves, until the core consumes its terminal result.
    pub(crate) fn abandon_owner_client_modeset_requester(
        &mut self,
        token: yserver_core::backend::CrtcConfigToken,
    ) -> Option<bool> {
        let (device, tag, phase) = self.lifecycle_drivers.iter().find_map(|(device, driver)| {
            driver
                .client_modeset
                .as_ref()
                .filter(|slot| slot.token == token)
                .map(|slot| (*device, slot.tag, slot.phase))
        })?;
        if phase == ClientModesetPhase::Dispatched {
            return Some(false);
        }

        if let Some(conductor) = self.admission_conductors.get_mut(&device) {
            conductor
                .admission
                .cancel_topology(TopologyWork::ClientModeset(tag));
        }

        let validation = self.lifecycle_drivers.get_mut(&device).and_then(|driver| {
            driver
                .pending_client_modeset_validations
                .iter_mut()
                .find(|(_, pending)| pending.tag == tag)
                .map(|(commit, pending)| {
                    pending.cancelled = true;
                    (*commit, pending.sent, pending.token.take())
                })
        });
        if let Some((commit, sent, admission_token)) = validation {
            if let Some(admission_token) = admission_token
                && let Some(conductor) = self.admission_conductors.get_mut(&device)
            {
                let _ = conductor.admission.abort(admission_token);
            }
            if !sent {
                if let Some(owner) = self.platform.owner_for(device) {
                    let _ = owner.abandon_validation(commit);
                }
                self.lifecycle_abort_client_modeset_validation(device, commit, false);
            }
        }

        if let Some(slot) = self.lifecycle_take_client_modeset_slot(device, tag) {
            self.lifecycle_release_client_modeset_slot(slot);
        }
        Some(true)
    }

    fn lifecycle_queue_waiting_client_modeset(&mut self, device: DrmDeviceKey) {
        let current = self
            .lifecycle_coordinator
            .device(&device)
            .is_some_and(|arbiter| {
                arbiter.state() == crate::kms::owner::lifecycle::DeviceLifecycleState::Ready
                    && arbiter.transition().is_none()
            });
        if !current {
            return;
        }
        let tag = self.lifecycle_drivers.get(&device).and_then(|driver| {
            let slot = driver.client_modeset.as_ref()?;
            (slot.phase == ClientModesetPhase::Queued).then_some(slot.tag)
        });
        let Some(tag) = tag else {
            return;
        };
        if let Some(conductor) = self.admission_conductors.get_mut(&device)
            && conductor.admission.topology().is_none()
        {
            let _ = conductor
                .admission
                .request_topology(TopologyWork::ClientModeset(tag));
        }
    }

    fn lifecycle_owner_devices(&self) -> Vec<DrmDeviceKey> {
        self.platform
            .devices
            .iter()
            .filter_map(|entry| {
                self.platform
                    .transport_gate(&entry.key)
                    .is_some_and(|gate| {
                        gate.state() == crate::kms::render::resources::TransportState::Owner
                    })
                    .then_some(entry.key)
            })
            .collect()
    }

    pub(super) fn lifecycle_register_owner_device(
        &mut self,
        device: DrmDeviceKey,
    ) -> Result<(), crate::kms::owner::lifecycle::CoordinatorError> {
        use crate::kms::owner::lifecycle::LifecycleArbiter;

        let Some(incarnation) = self
            .platform
            .owner_ref(device)
            .map(|owner| owner.incarnation())
        else {
            return Err(crate::kms::owner::lifecycle::CoordinatorError::UnknownDevice);
        };
        self.owner_dpms_installed_active
            .entry(device)
            .or_insert(true);
        if !self.admission_conductors.contains_key(&device) {
            self.install_admission_conductor(device, Box::new(LifecycleOnlyAdmissionSource));
        }
        self.lifecycle_drivers
            .entry(device)
            .or_insert_with(LifecycleDriver::new);
        if self.lifecycle_coordinator.device(&device).is_none() {
            let dispatches = self
                .lifecycle_coordinator
                .add_device(device, LifecycleArbiter::new(incarnation))?;
            for dispatch in dispatches {
                self.lifecycle_queue_actions(
                    dispatch.device,
                    dispatch.actions,
                    self.lifecycle_current_tag(dispatch.device),
                );
            }
        }

        let outputs = self
            .platform
            .outputs
            .iter()
            .filter(|output| output.key.device_key == device)
            .map(|output| output.key.clone())
            .collect::<Vec<_>>();
        let current_outputs: std::collections::HashSet<_> = outputs.iter().cloned().collect();
        let removed_outputs = self
            .lifecycle_coordinator
            .device(&device)
            .into_iter()
            .flat_map(|arbiter| arbiter.desired().dpms_targets().keys())
            .filter(|output| !current_outputs.contains(*output))
            .cloned()
            .collect::<Vec<_>>();
        for output in removed_outputs {
            self.lifecycle_coordinator
                .remove_protocol_output(&device, &output)?;
        }
        for output in outputs {
            let addition = self
                .lifecycle_coordinator
                .add_protocol_output(&device, output)?;
            if let Some(dispatch) = addition.synchronization {
                self.lifecycle_queue_actions(
                    dispatch.device,
                    dispatch.actions,
                    self.lifecycle_current_tag(dispatch.device),
                );
            }
        }
        Ok(())
    }

    /// Project one protocol DPMS request to Owner devices and drain each
    /// device's run-to-completion queue before returning. `false` leaves the
    /// existing all-Legacy path byte-for-byte in control.
    pub(crate) fn lifecycle_set_dpms_power(&mut self, level: u8) -> std::io::Result<bool> {
        let owner_devices = self.lifecycle_owner_devices();
        if owner_devices.is_empty() {
            return Ok(false);
        }
        for device in owner_devices.iter().copied() {
            self.lifecycle_register_owner_device(device)
                .map_err(|error| {
                    std::io::Error::other(format!("lifecycle registration: {error:?}"))
                })?;
        }
        let dispatches = self
            .lifecycle_coordinator
            .set_protocol_dpms_level(level)
            .map_err(|error| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!("lifecycle DPMS: {error:?}"),
                )
            })?;
        for dispatch in dispatches {
            let requester = self.lifecycle_current_tag(dispatch.device);
            self.lifecycle_queue_actions(dispatch.device, dispatch.actions, requester);
        }
        for device in owner_devices {
            self.lifecycle_drain_driver(device);
        }
        Ok(true)
    }

    /// Observe the seat target from the existing VT paths. This is only a
    /// prerequisite feed for Owner lifecycle work; the existing suspend and
    /// resume code remains the sole Legacy VT executor in stage 3a.
    pub(crate) fn lifecycle_observe_seat_target(
        &mut self,
        target: crate::kms::owner::lifecycle::SeatTarget,
    ) {
        let owner_devices = self.lifecycle_owner_devices();
        for device in owner_devices.iter().copied() {
            if let Err(error) = self.lifecycle_register_owner_device(device) {
                log::error!("lifecycle seat observation registration for {device:?}: {error:?}");
            }
        }
        let dispatches = match self.lifecycle_coordinator.observe_seat_target(target) {
            Ok(dispatches) => dispatches,
            Err(error) => {
                log::error!("lifecycle seat observation failed: {error:?}");
                return;
            }
        };
        for dispatch in dispatches {
            let requester = self.lifecycle_current_tag(dispatch.device);
            self.lifecycle_queue_actions(dispatch.device, dispatch.actions, requester);
        }
        for device in owner_devices {
            self.lifecycle_drain_driver(device);
        }
    }

    fn lifecycle_current_tag(&self, device: DrmDeviceKey) -> Option<TransitionTag<IncarnationId>> {
        self.lifecycle_coordinator
            .device(&device)
            .and_then(|arbiter| arbiter.transition_tag())
    }

    fn lifecycle_queue_actions(
        &mut self,
        device: DrmDeviceKey,
        actions: Vec<LifecycleAction<IncarnationId>>,
        requester: Option<TransitionTag<IncarnationId>>,
    ) {
        if actions.is_empty() {
            return;
        }
        if actions
            .iter()
            .any(|action| matches!(action, LifecycleAction::CloseAdmission(_)))
        {
            self.lifecycle_cancel_client_modeset_before_dispatch(
                device,
                self.lifecycle_superseding_kind(device),
            );
        }
        let Some(driver) = self.lifecycle_drivers.get_mut(&device) else {
            return;
        };
        driver.enqueue(LifecycleDriverWork::Actions { requester, actions });
        self.lifecycle_drain_driver(device);
    }

    fn lifecycle_queue_input(&mut self, device: DrmDeviceKey, input: ArbiterInput<IncarnationId>) {
        if let Some(driver) = self.lifecycle_drivers.get_mut(&device) {
            driver.enqueue(LifecycleDriverWork::Input(input));
            self.lifecycle_drain_driver(device);
        }
    }

    fn lifecycle_drain_driver(&mut self, device: DrmDeviceKey) {
        let Some(driver) = self.lifecycle_drivers.get_mut(&device) else {
            return;
        };
        if driver.draining {
            return;
        }
        if driver.routing_batch_depth != 0 {
            return;
        }
        driver.draining = true;
        #[cfg(test)]
        {
            driver.drain_entries = driver.drain_entries.saturating_add(1);
        }

        loop {
            self.promote_waiting_clock_probe(device);
            let Some(work) = self
                .lifecycle_drivers
                .get_mut(&device)
                .and_then(LifecycleDriver::pop)
            else {
                break;
            };
            match work {
                LifecycleDriverWork::Actions { requester, actions } => {
                    for action in actions {
                        self.lifecycle_apply_action(device, requester, action);
                    }
                }
                LifecycleDriverWork::Input(input) => {
                    #[cfg(test)]
                    let current_tag = self.lifecycle_current_tag(device);
                    #[cfg(test)]
                    if let Some(driver) = self.lifecycle_drivers.get_mut(&device) {
                        driver.applied_inputs = driver.applied_inputs.saturating_add(1);
                        if matches!(input, ArbiterInput::Receipt { .. })
                            && current_tag.is_some_and(|current| {
                                matches!(input, ArbiterInput::Receipt { tag, .. } if tag != current)
                            })
                        {
                            driver.stale_receipts = driver.stale_receipts.saturating_add(1);
                        }
                    }
                    let actions = self
                        .lifecycle_coordinator
                        .apply_device_input(&device, input)
                        .unwrap_or_default();
                    let requester = self.lifecycle_current_tag(device);
                    if !actions.is_empty()
                        && let Some(driver) = self.lifecycle_drivers.get_mut(&device)
                    {
                        driver.enqueue(LifecycleDriverWork::Actions { requester, actions });
                    }
                }
                LifecycleDriverWork::ValidationResolved { commit, outcome } => {
                    self.lifecycle_finish_topology_validation(device, commit, outcome);
                }
                LifecycleDriverWork::TopologyTerminal {
                    commit,
                    tag,
                    terminal,
                } => {
                    self.lifecycle_dispose_topology_result(device, commit, tag, terminal);
                }
                LifecycleDriverWork::ClientModesetTerminal {
                    commit,
                    tag,
                    terminal,
                } => {
                    self.lifecycle_finish_client_modeset(device, commit, tag, terminal);
                }
            }
        }
        if let Some(driver) = self.lifecycle_drivers.get_mut(&device) {
            driver.draining = false;
        }
    }

    fn lifecycle_apply_action(
        &mut self,
        device: DrmDeviceKey,
        requester: Option<TransitionTag<IncarnationId>>,
        action: LifecycleAction<IncarnationId>,
    ) {
        let action_tag = match &action {
            LifecycleAction::StopAliasCreation(tag)
            | LifecycleAction::CancelPreSubmitWork(tag)
            | LifecycleAction::AwaitTerminalState(tag)
            | LifecycleAction::TerminalizePresents(tag)
            | LifecycleAction::TransferQuarantineToWinner(tag)
            | LifecycleAction::PhysicalAdvanceAllowed(tag) => Some(*tag),
            _ => requester,
        };
        match action {
            LifecycleAction::CloseAdmission(work_tag) => {
                if let Some(conductor) = self.admission_conductors.get_mut(&device) {
                    conductor.lifecycle_admission_closed = true;
                }
                if let Some(tag) = action_tag {
                    self.lifecycle_sync_owner_tag(device, tag);
                    self.lifecycle_receipt(
                        device,
                        tag,
                        LifecycleReceipt::AdmissionClosed,
                        LifecycleReceiptResult::Succeeded,
                    );
                } else {
                    log::error!("lifecycle close admission lacks transition tag: {work_tag:?}");
                }
            }
            LifecycleAction::ReopenAdmission(work_tag) => {
                let can_reopen = self
                    .lifecycle_coordinator
                    .device(&device)
                    .is_some_and(|arbiter| arbiter.admission_open());
                if can_reopen {
                    if let Some(conductor) = self.admission_conductors.get_mut(&device) {
                        conductor.lifecycle_admission_closed = false;
                    }
                    if let Some(owner) = self.platform.owner_for(device) {
                        let _ = owner.update_lifecycle_context(work_tag.lifecycle_epoch, None);
                    }
                    if self.owner_outputs_powered_on(device)
                        && self.lifecycle_coordinator.protocol_dpms_level() == 0
                    {
                        self.scene.wake_for_devices(
                            &self.platform,
                            &std::collections::HashSet::from([device]),
                        );
                    }
                    self.lifecycle_queue_waiting_client_modeset(device);
                    self.admission_wake(device, false);
                }
            }
            LifecycleAction::StopAliasCreation(_tag) => {}
            LifecycleAction::CancelPreSubmitWork(tag) => {
                let succeeded = self.lifecycle_cancel_pre_submit(device, tag);
                self.lifecycle_receipt(
                    device,
                    tag,
                    LifecycleReceipt::PreSubmitWorkCancelled,
                    if succeeded {
                        LifecycleReceiptResult::Succeeded
                    } else {
                        LifecycleReceiptResult::Failed
                    },
                );
            }
            LifecycleAction::AwaitTerminalState(_tag) => {}
            LifecycleAction::TerminalizePresents(tag) => {
                let succeeded = self.lifecycle_terminalize_presents(device);
                self.lifecycle_receipt(
                    device,
                    tag,
                    LifecycleReceipt::PresentsTerminalized,
                    if succeeded {
                        LifecycleReceiptResult::Succeeded
                    } else {
                        LifecycleReceiptResult::Failed
                    },
                );
            }
            LifecycleAction::TransferQuarantineToWinner(tag) => {
                // Owner/CommitConsumer already own completion-unknown records;
                // there is no detachable quarantined state in this stage.
                self.lifecycle_receipt(
                    device,
                    tag,
                    LifecycleReceipt::QuarantineTransferred,
                    LifecycleReceiptResult::Succeeded,
                );
            }
            LifecycleAction::PhysicalAdvanceAllowed(tag) => {
                if !self.lifecycle_tag_current(device, tag) {
                    return;
                }
                if let Some(conductor) = self.admission_conductors.get_mut(&device) {
                    let _ = conductor
                        .admission
                        .request_topology(TopologyWork::Transition(tag));
                }
                self.lifecycle_sync_owner_tag(device, tag);
                self.admission_wake(device, false);
            }
            LifecycleAction::EpochAdvanced(epoch) => {
                let current = self
                    .lifecycle_coordinator
                    .device(&device)
                    .and_then(|arbiter| arbiter.transition());
                let transition = current.map(|transition| transition.id);
                let context_updated = self
                    .platform
                    .owner_for(device)
                    .is_some_and(|owner| owner.update_lifecycle_context(epoch, transition));
                if context_updated {
                    if current.is_some_and(|transition| {
                        transition.kind == crate::kms::owner::lifecycle::LifecycleKind::DPMS
                    }) && let Some(owner) = self.platform.owner_for(device)
                    {
                        owner.carry_resolved_clock_context(epoch);
                    }
                    self.activate_admission_clock_probes(device);
                }
            }
            LifecycleAction::DispositionChanged { .. }
            | LifecycleAction::ReleaseSeat(_)
            | LifecycleAction::WithdrawOutputs(_)
            | LifecycleAction::TerminalizeProtocolWork(_)
            | LifecycleAction::AllocateRecoveryIncident { .. }
            | LifecycleAction::RecoveryTableF(_)
            | LifecycleAction::CompletionLossTableU { .. }
            | LifecycleAction::CompletionBarriersRequired(_)
            | LifecycleAction::ReceiptFailed { .. }
            | LifecycleAction::StaleReceiptIgnored { .. }
            | LifecycleAction::StaleResultIgnored { .. }
            | LifecycleAction::OrdinaryReplyCurrent
            | LifecycleAction::TransitionIdExhausted
            | LifecycleAction::EpochExhausted
            | LifecycleAction::MissingBoundaryRecoveryId(_) => {}
        }
    }

    fn lifecycle_receipt(
        &mut self,
        device: DrmDeviceKey,
        tag: TransitionTag<IncarnationId>,
        receipt: LifecycleReceipt,
        result: LifecycleReceiptResult,
    ) {
        let input = ArbiterInput::Receipt {
            tag,
            receipt,
            result,
        };
        if let Some(driver) = self.lifecycle_drivers.get_mut(&device) {
            driver.defer_receipt(input);
            #[cfg(not(test))]
            self.lifecycle_drain_driver(device);
        }
    }

    fn lifecycle_sync_owner_tag(
        &mut self,
        device: DrmDeviceKey,
        tag: TransitionTag<IncarnationId>,
    ) {
        if self
            .lifecycle_coordinator
            .device(&device)
            .and_then(|arbiter| arbiter.transition_tag())
            != Some(tag)
        {
            return;
        }
        if let Some(owner) = self.platform.owner_for(device)
            && owner.incarnation() == tag.incarnation
        {
            let _ = owner.update_lifecycle_context(tag.lifecycle_epoch, Some(tag.transition));
        }
    }

    fn lifecycle_tag_current(
        &self,
        device: DrmDeviceKey,
        tag: TransitionTag<IncarnationId>,
    ) -> bool {
        self.lifecycle_coordinator
            .device(&device)
            .and_then(|arbiter| arbiter.transition_tag())
            == Some(tag)
            && self.platform.owner_ref(device).is_some_and(|owner| {
                owner.incarnation() == tag.incarnation
                    && owner.lifecycle_epoch() == tag.lifecycle_epoch
                    && owner.lifecycle_transition() == Some(tag.transition)
            })
    }

    pub(crate) fn client_modeset_tag_current(
        &self,
        device: DrmDeviceKey,
        tag: ClientModesetTag<IncarnationId>,
    ) -> bool {
        self.platform.transport_gate(&device).is_some_and(|gate| {
            gate.state() == crate::kms::render::resources::TransportState::Owner
        }) && self
            .lifecycle_drivers
            .get(&device)
            .and_then(|driver| driver.client_modeset.as_ref())
            .is_some_and(|slot| slot.tag == tag)
            && self.platform.owner_ref(device).is_some_and(|owner| {
                owner.incarnation() == tag.incarnation
                    && owner.lifecycle_epoch() == tag.lifecycle_epoch
                    && owner.topology_generation() == tag.topology_generation
            })
            && self
                .lifecycle_coordinator
                .device(&device)
                .is_some_and(|arbiter| {
                    arbiter.incarnation() == &tag.incarnation
                        && arbiter.epoch() == tag.lifecycle_epoch
                })
    }

    fn client_modeset_projection_current(
        &self,
        device: DrmDeviceKey,
        projection: Option<&StagedDpmsProjection>,
    ) -> bool {
        projection.is_none_or(|projection| {
            projection.output.device_key == device
                && self.lifecycle_coordinator.protocol_dpms_level() == projection.level
                && self.lifecycle_coordinator.dpms_epoch() == projection.epoch
        })
    }

    fn client_modeset_displaced_pool(
        &self,
        device: DrmDeviceKey,
        connector: &str,
        target_crtc: u32,
    ) -> Result<
        Option<(
            GroupMember,
            Vec<crate::kms::render::resources::AllocationKey>,
        )>,
        ResourceError,
    > {
        let Some(output_idx) = self.platform.outputs.iter().position(|output| {
            output.key == crate::kms::backend::OutputKey::new(device, connector)
        }) else {
            return Ok(None);
        };
        let output = &self.platform.outputs[output_idx];
        let crtc = CrtcKey::for_output(output);
        if u32::from(crtc.crtc) != target_crtc {
            return Err(ResourceError::InvalidState);
        }
        let scanout = self
            .platform
            .scanout_pools
            .get(output_idx)
            .and_then(Option::as_ref)
            .ok_or(ResourceError::InvalidState)?;
        if !matches!(
            scanout,
            crate::kms::vk::scanout::OutputScanout::Shared(_)
                | crate::kms::vk::scanout::OutputScanout::Copied(_)
        ) {
            return Err(ResourceError::InvalidState);
        }
        let allocations = scanout
            .display_pool()
            .bos
            .iter()
            .map(|bo| bo.managed_key().ok_or(ResourceError::InvalidState))
            .collect::<Result<Vec<_>, _>>()?;
        let owner = self
            .platform
            .owner_ref(device)
            .ok_or(ResourceError::InvalidState)?;
        let clock_epoch = owner
            .clock_key_for_hardware_crtc(target_crtc)
            .ok_or(ResourceError::InvalidState)?
            .epoch
            .get();
        let member = GroupMember::new(crtc, owner.topology_generation(), clock_epoch);
        Ok(Some((member, allocations)))
    }

    pub(crate) fn lifecycle_complete_kms_displacements(
        &mut self,
        device: DrmDeviceKey,
        commit: CommitId,
        retired_members: &[GroupMember],
    ) {
        let Some(incarnation) = self
            .platform
            .owner_ref(device)
            .map(|owner| owner.incarnation())
        else {
            return;
        };
        self.lifecycle_record_untracked_retired_crtcs(device, commit, incarnation, retired_members);
        let (fenced_crtcs, dark_proofs, registrations) = {
            let Some(driver) = self.lifecycle_drivers.get(&device) else {
                return;
            };
            let stale_client = driver
                .client_modeset_commits
                .get(&commit)
                .is_some_and(|tag| !self.client_modeset_tag_current(device, *tag));
            let stale_transition = driver
                .topology_commits
                .get(&commit)
                .is_some_and(|tag| !self.lifecycle_tag_current(device, *tag));
            if stale_client || stale_transition {
                return;
            }

            let mut fenced_crtcs = retired_members
                .iter()
                .map(|member| u32::from(member.crtc.crtc))
                .collect::<HashSet<_>>();
            let changes = driver
                .owner_commit_power_changes
                .get(&commit)
                .map(Vec::as_slice)
                .unwrap_or_default();
            fenced_crtcs.extend(
                changes
                    .iter()
                    .filter(|change| {
                        change.incarnation == incarnation && change.expected_completion
                    })
                    .map(|change| change.crtc),
            );
            let mut dark_proofs = BTreeMap::new();
            for change in changes.iter().filter(|change| {
                change.incarnation == incarnation
                    && !change.new_active
                    && !change.expected_completion
            }) {
                if let Some(InstalledCrtcPower::InactiveProven {
                    incarnation: proof_incarnation,
                    off_commit,
                }) = driver.installed_crtc_power.get(&change.crtc)
                    && *proof_incarnation == incarnation
                    && *off_commit <= commit
                {
                    let Some(handle) = ::drm::control::from_u32(change.crtc) else {
                        continue;
                    };
                    dark_proofs.insert(
                        change.crtc,
                        crate::kms::render::resources::DarkCrtcDisplacement {
                            off_commit: *off_commit,
                            crtc: CrtcKey::new(device, handle),
                        },
                    );
                }
            }
            let registrations = driver
                .kms_displacements
                .iter()
                .flat_map(|(&registered_commit, registrations)| {
                    registrations
                        .iter()
                        .copied()
                        .map(move |registration| (registered_commit, registration))
                })
                .filter(|(registered_commit, registration)| {
                    *registered_commit <= commit
                        && registration.commit <= commit
                        && registration.allocation.incarnation == incarnation
                })
                .collect::<Vec<_>>();
            (fenced_crtcs, dark_proofs, registrations)
        };

        let mut discharged = Vec::new();
        {
            let Some(service) = self.resource_service.as_mut() else {
                return;
            };
            for (registered_commit, registration) in registrations {
                let crtc_id = u32::from(registration.member.crtc.crtc);
                let proof = if fenced_crtcs.contains(&crtc_id) {
                    Some(
                        crate::kms::render::resources::KmsReleaseProof::CompletionRetired {
                            through_commit: commit,
                            crtc: registration.member.crtc,
                        },
                    )
                } else {
                    dark_proofs.get(&crtc_id).copied().map(|proof| {
                        crate::kms::render::resources::KmsReleaseProof::DarkCrtcDisplacement {
                            through_commit: commit,
                            proof,
                        }
                    })
                };
                let Some(proof) = proof else {
                    continue;
                };
                match service.discharge_kms_release(registration, proof) {
                    Ok(()) => discharged.push((registered_commit, registration)),
                    Err(error) => log::error!(
                        "Owner KMS displacement proof refused for {device:?} commit {commit:?} allocation {:?}: {error:?}",
                        registration.allocation
                    ),
                }
            }
        }
        for (_, registration) in &discharged {
            self.scene.retire_retired_pool_bo_after_kms_proof(
                registration.allocation,
                &mut self.platform,
            );
        }
        if let Some(driver) = self.lifecycle_drivers.get_mut(&device) {
            for (registered_commit, discharged) in discharged {
                let mut remove_entry = false;
                if let Some(registrations) = driver.kms_displacements.get_mut(&registered_commit) {
                    registrations.retain(|registration| *registration != discharged);
                    remove_entry = registrations.is_empty();
                }
                if remove_entry {
                    driver.kms_displacements.remove(&registered_commit);
                }
            }
        }
    }

    fn lifecycle_record_untracked_retired_crtcs(
        &mut self,
        device: DrmDeviceKey,
        commit: CommitId,
        incarnation: IncarnationId,
        retired_members: &[GroupMember],
    ) {
        let Some(driver) = self.lifecycle_drivers.get_mut(&device) else {
            return;
        };
        let tracked_crtcs = driver
            .owner_commit_power_changes
            .get(&commit)
            .into_iter()
            .flatten()
            .filter(|change| change.incarnation == incarnation)
            .map(|change| change.crtc)
            .collect::<HashSet<_>>();
        for member in retired_members.iter().filter(|member| {
            member.crtc.device_key == device
                && !tracked_crtcs.contains(&u32::from(member.crtc.crtc))
        }) {
            // Resource-bearing non-lifecycle Owner commits carry only lit
            // composed work. Seeing one retire on a CRTC invalidates any
            // earlier OFF chain that this driver did not track explicitly.
            driver.installed_crtc_power.insert(
                u32::from(member.crtc.crtc),
                InstalledCrtcPower::Active { incarnation },
            );
        }
    }

    pub(crate) fn lifecycle_cancel_kms_displacements(
        &mut self,
        device: DrmDeviceKey,
        commit: CommitId,
    ) {
        let registrations = self
            .lifecycle_drivers
            .get_mut(&device)
            .and_then(|driver| driver.kms_displacements.remove(&commit))
            .unwrap_or_default();
        let Some(service) = self.resource_service.as_mut() else {
            if !registrations.is_empty() {
                log::error!(
                    "Owner KMS displacement cancellation has no resource service for {device:?}"
                );
            }
            return;
        };
        for registration in registrations {
            if let Err(error) = service.cancel(registration.allocation, registration.obligation) {
                log::error!(
                    "Owner KMS displacement cancellation refused for {device:?} commit {commit:?} allocation {:?}: {error:?}",
                    registration.allocation
                );
            }
        }
    }

    pub(crate) fn lifecycle_update_installed_crtc_power(
        &mut self,
        device: DrmDeviceKey,
        commit: CommitId,
        terminal: TerminalState,
    ) {
        let Some(incarnation) = self
            .platform
            .owner_ref(device)
            .map(|owner| owner.incarnation())
        else {
            return;
        };
        let Some(driver) = self.lifecycle_drivers.get_mut(&device) else {
            return;
        };
        let changes = driver
            .owner_commit_power_changes
            .remove(&commit)
            .unwrap_or_default();
        match terminal {
            TerminalState::Completed => {
                for change in changes {
                    if change.incarnation != incarnation {
                        continue;
                    }
                    let power = if change.new_active {
                        InstalledCrtcPower::Active { incarnation }
                    } else if change.expected_completion {
                        InstalledCrtcPower::InactiveProven {
                            incarnation,
                            off_commit: commit,
                        }
                    } else if matches!(
                        driver.installed_crtc_power.get(&change.crtc),
                        Some(InstalledCrtcPower::InactiveProven {
                            incarnation: known,
                            ..
                        }) if *known == incarnation
                    ) {
                        *driver
                            .installed_crtc_power
                            .get(&change.crtc)
                            .expect("the inactive proof was just checked")
                    } else {
                        InstalledCrtcPower::InactiveUnproven { incarnation }
                    };
                    driver.installed_crtc_power.insert(change.crtc, power);
                }
            }
            TerminalState::FailedBeforeSubmit(_) => {}
            TerminalState::CompletionUnknown(_) => {
                for power in driver.installed_crtc_power.values_mut() {
                    *power = InstalledCrtcPower::Unknown { incarnation };
                }
                for change in changes {
                    driver
                        .installed_crtc_power
                        .insert(change.crtc, InstalledCrtcPower::Unknown { incarnation });
                }
            }
        }
    }

    fn lifecycle_superseding_kind(
        &self,
        device: DrmDeviceKey,
    ) -> crate::kms::owner::lifecycle::LifecycleKind {
        self.lifecycle_coordinator
            .device(&device)
            .and_then(|arbiter| arbiter.transition())
            .map_or(
                crate::kms::owner::lifecycle::LifecycleKind::DPMS,
                |transition| transition.kind,
            )
    }

    fn lifecycle_take_client_modeset_slot(
        &mut self,
        device: DrmDeviceKey,
        tag: ClientModesetTag<IncarnationId>,
    ) -> Option<ClientModesetSlot> {
        let driver = self.lifecycle_drivers.get_mut(&device)?;
        driver
            .client_modeset
            .as_ref()
            .is_some_and(|slot| slot.tag == tag)
            .then(|| driver.client_modeset.take().expect("slot checked"))
    }

    fn lifecycle_release_client_modeset_slot(&mut self, mut slot: ClientModesetSlot) {
        let Some(prepared) = slot.prepared.take() else {
            return;
        };
        #[cfg(test)]
        self.client_modeset_released_allocation_keys_for_tests
            .extend(prepared.prepared_set.allocation_keys.iter().copied());
        #[cfg(test)]
        self.client_modeset_released_framebuffers_for_tests.extend(
            prepared
                .prepared_set
                .framebuffer_handles_for_tests
                .iter()
                .copied(),
        );
        match (
            self.resource_service.as_mut(),
            self.drm_cleanup_registry.as_mut(),
        ) {
            (Some(service), Some(registry)) => prepared.prepared_set.release(service, registry),
            _ => log::error!(
                "prepared client modeset {:?} lost its 2c-i service before release",
                slot.tag
            ),
        }
    }

    fn lifecycle_complete_queued_client_modeset(
        &mut self,
        device: DrmDeviceKey,
        tag: ClientModesetTag<IncarnationId>,
        kind: crate::kms::owner::lifecycle::LifecycleKind,
    ) {
        if let Some(conductor) = self.admission_conductors.get_mut(&device) {
            conductor
                .admission
                .cancel_topology(TopologyWork::ClientModeset(tag));
        }
        let slot = self.lifecycle_take_client_modeset_slot(device, tag);
        if let Some(slot) = slot {
            let token = slot.token;
            let result = client_modeset_error_for_slot(
                device,
                &slot,
                Err(std::io::Error::other(ClientModesetFailure::Superseded(
                    kind,
                ))),
            );
            self.lifecycle_release_client_modeset_slot(slot);
            self.complete_owner_client_modeset(token, result);
        }
        self.lifecycle_end_client_modeset_direct_hold(device, tag, false);
    }

    fn lifecycle_cancel_client_modeset_before_dispatch(
        &mut self,
        device: DrmDeviceKey,
        kind: crate::kms::owner::lifecycle::LifecycleKind,
    ) -> bool {
        let Some((tag, phase)) = self
            .lifecycle_drivers
            .get(&device)
            .and_then(|driver| driver.client_modeset.as_ref())
            .map(|slot| (slot.tag, slot.phase))
        else {
            return true;
        };
        match phase {
            ClientModesetPhase::Queued => {
                self.lifecycle_complete_queued_client_modeset(device, tag, kind);
                true
            }
            ClientModesetPhase::Validating => {
                let pending_commit = self.lifecycle_drivers.get_mut(&device).and_then(|driver| {
                    driver
                        .pending_client_modeset_validations
                        .iter_mut()
                        .find(|(_, pending)| pending.tag == tag)
                        .map(|(commit, pending)| {
                            pending.cancelled = true;
                            (*commit, pending.sent)
                        })
                });
                if let Some((commit, false)) = pending_commit {
                    let abandoned = self
                        .platform
                        .owner_for(device)
                        .is_some_and(|owner| owner.abandon_validation(commit).is_ok());
                    if abandoned {
                        self.lifecycle_finish_client_modeset_validation_cancelled(
                            device, commit, tag,
                        );
                    }
                    abandoned
                } else {
                    true
                }
            }
            ClientModesetPhase::Dispatched => true,
        }
    }

    fn lifecycle_cancel_pre_submit(
        &mut self,
        device: DrmDeviceKey,
        tag: TransitionTag<IncarnationId>,
    ) -> bool {
        let mut success = true;
        if let Some(conductor) = self.admission_conductors.get_mut(&device) {
            conductor
                .admission
                .cancel_topology(TopologyWork::Transition(tag));
        }
        let validation = self.lifecycle_drivers.get_mut(&device).and_then(|driver| {
            driver
                .pending_topology_validations
                .iter_mut()
                .find(|(_, pending)| pending.tag == tag)
                .map(|(commit, pending)| (*commit, pending))
        });
        if let Some((commit, pending)) = validation {
            pending.cancelled = true;
            if !pending.sent {
                if let Some(owner) = self.platform.owner_for(device) {
                    success &= owner.abandon_validation(commit).is_ok();
                }
                if let Some(token) = self
                    .lifecycle_drivers
                    .get_mut(&device)
                    .and_then(|driver| driver.pending_topology_validations.remove(&commit))
                    .and_then(|pending| pending.token)
                    && let Some(conductor) = self.admission_conductors.get_mut(&device)
                {
                    success &= conductor.admission.abort(token).is_ok();
                }
            }
        }
        success &= self.lifecycle_cancel_client_modeset_before_dispatch(
            device,
            self.lifecycle_superseding_kind(device),
        );
        success
    }

    fn lifecycle_terminalize_presents(&mut self, device: DrmDeviceKey) -> bool {
        let source_generation = self
            .admission_conductors
            .get(&device)
            .and_then(|conductor| conductor.admission.direct())
            .map(|queued| queued.successor.source_generation);
        if let Some(source_generation) = source_generation {
            if let Some(conductor) = self.admission_conductors.get_mut(&device) {
                conductor.admission.withdraw_direct(source_generation);
            }
            return self.managed_terminalize_queued_direct_successor(Some(source_generation));
        }
        let _ = self.managed_terminalize_queued_direct_successor(None);
        true
    }

    fn lifecycle_prepare_client_modeset(
        &mut self,
        device: DrmDeviceKey,
        tag: ClientModesetTag<IncarnationId>,
    ) -> std::io::Result<PreparedClientModesetDescription> {
        use crate::{
            drm::modeset::PropMap,
            kms::render::{
                admission::PreparationStage as Stage,
                composed_commit::{ComposedPlane, discover_composed_property_ids},
            },
        };
        use ::drm::control::Device as _;

        let preparation_error =
            |stage| std::io::Error::other(ClientModesetFailure::Preparation(stage));
        let request = self
            .lifecycle_drivers
            .get(&device)
            .and_then(|driver| driver.client_modeset.as_ref())
            .filter(|slot| slot.tag == tag)
            .map(|slot| {
                (
                    slot.output_id,
                    slot.connector.clone(),
                    slot.mode,
                    slot.x,
                    slot.y,
                )
            })
            .ok_or_else(|| std::io::Error::other("client modeset slot became stale"))?;
        let output_key = self
            .output_key_for_id(request.0)
            .filter(|key| key.device_key == device && key.connector_name == request.1)
            .cloned()
            .ok_or_else(|| preparation_error(Stage::Discovery))?;
        let current = self
            .platform
            .outputs
            .iter()
            .find(|output| output.key == output_key)
            .map(|output| {
                (
                    u32::from(output.output.crtc),
                    u32::from(output.output.plane),
                )
            });
        let requested_mode = match request.2 {
            Some(mode) => mode,
            None => self
                .platform
                .outputs
                .iter()
                .find(|output| output.key == output_key)
                .map(|output| yserver_core::backend::ModeSpec {
                    width: output.width,
                    height: output.height,
                    vrefresh: output.output.picked.vrefresh,
                })
                .ok_or_else(|| preparation_error(Stage::Discovery))?,
        };
        if requested_mode.width == 0 || requested_mode.height == 0 {
            return Err(preparation_error(Stage::Discovery));
        }

        let drm_device = self
            .platform
            .device_for_key(device)
            .map(|entry| std::rc::Rc::clone(&entry.device))
            .ok_or_else(|| preparation_error(Stage::Discovery))?;
        let reserved_routes = self
            .platform
            .outputs
            .iter()
            .filter(|output| output.key.device_key == device && output.key != output_key)
            .map(|output| {
                (
                    output.output.encoder,
                    output.output.crtc,
                    output.output.plane,
                )
            })
            .collect::<Vec<_>>();
        let discovered = crate::drm::modeset::discover_output_for_connector(
            &drm_device,
            &request.1,
            &reserved_routes,
        )
        .map_err(|_| preparation_error(Stage::Discovery))?;
        #[cfg(test)]
        let discovered = if let Some((crtc, plane)) = self
            .client_modeset_discovery_route_override_for_tests
            .take()
        {
            let mut discovered = discovered;
            discovered.crtc =
                ::drm::control::from_u32(crtc).expect("test discovery CRTC is nonzero");
            discovered.plane =
                ::drm::control::from_u32(plane).expect("test discovery plane is nonzero");
            discovered
        } else {
            discovered
        };
        if discovered.connector_name != request.1 {
            return Err(preparation_error(Stage::Discovery));
        }
        if !discovered.modes.iter().any(|candidate| {
            candidate.width == requested_mode.width
                && candidate.height == requested_mode.height
                && candidate.vrefresh == requested_mode.vrefresh
        }) {
            return Err(preparation_error(Stage::Mode));
        }
        if let Some((bound_crtc, bound_plane)) = current
            && (u32::from(discovered.crtc) != bound_crtc
                || u32::from(discovered.plane) != bound_plane)
        {
            return Err(preparation_error(Stage::Route));
        }
        let output = crate::drm::modeset::output_for_exact_probe_assignment(
            &drm_device,
            discovered.connector,
            discovered.encoder,
            discovered.crtc,
            discovered.plane,
            requested_mode,
        )
        .map_err(|_| preparation_error(Stage::Mode))?;

        let scanout_route = self
            .platform
            .scanout_route_for_kms(device)
            .map_err(|_| preparation_error(Stage::Route))?;
        if scanout_route.relationship == crate::kms::scanout_route::RenderKmsRelationship::Unknown {
            return Err(preparation_error(Stage::Route));
        }

        let global_level = self.lifecycle_coordinator.protocol_dpms_level();
        let global_epoch = self.lifecycle_coordinator.dpms_epoch();
        let device_desired = self
            .lifecycle_coordinator
            .device(&device)
            .ok_or_else(|| preparation_error(Stage::SceneState))?
            .desired();
        if request.2.is_none() && !device_desired.dpms_targets().contains_key(&output_key) {
            return Err(preparation_error(Stage::SceneState));
        }
        if let Some(current) = device_desired.dpms_targets().get(&output_key)
            && (current.level != global_level || current.epoch.unwrap_or(0) != global_epoch)
        {
            return Err(preparation_error(Stage::SceneState));
        }
        let projection = request
            .2
            .map(|_| {
                stage_dpms_projection(output_key.clone(), global_level, global_epoch)
                    .map_err(|_| preparation_error(Stage::SceneState))
            })
            .transpose()?;

        let property_ids = {
            let device_entry = self
                .platform
                .devices
                .iter_mut()
                .find(|entry| entry.key == device)
                .ok_or_else(|| preparation_error(Stage::Discovery))?;
            let common = discover_composed_property_ids(
                &drm_device,
                &[ComposedPlane {
                    output: &output,
                    framebuffer: ::drm::control::from_u32(1)
                        .expect("nonzero property-discovery framebuffer"),
                }],
                &mut device_entry.active_property_cache,
            )
            .map_err(|_| preparation_error(Stage::Discovery))?;
            let connector_crtc_id = u32::from(
                PropMap::for_object(&drm_device, output.connector)
                    .and_then(|properties| properties.id("CRTC_ID"))
                    .map_err(|_| preparation_error(Stage::Discovery))?,
            );
            let crtc_mode_id = u32::from(
                PropMap::for_object(&drm_device, output.crtc)
                    .and_then(|properties| properties.id("MODE_ID"))
                    .map_err(|_| preparation_error(Stage::Discovery))?,
            );
            ClientModesetPropertyIds {
                connector_crtc_id,
                crtc_mode_id,
                plane_fb_id: u32::from(output.plane_fb_id_prop),
                plane_crtc_id: u32::from(output.plane_crtc_id_prop),
                plane_src_x: u32::from(output.plane_src_x_prop),
                plane_src_y: u32::from(output.plane_src_y_prop),
                plane_src_w: u32::from(output.plane_src_w_prop),
                plane_src_h: u32::from(output.plane_src_h_prop),
                plane_crtc_x: u32::from(output.plane_crtc_x_prop),
                plane_crtc_y: u32::from(output.plane_crtc_y_prop),
                plane_crtc_w: u32::from(output.plane_crtc_w_prop),
                plane_crtc_h: u32::from(output.plane_crtc_h_prop),
                common,
            }
        };
        let old_crtc_id = current.map_or(0, |(crtc, _)| crtc);
        let old_active = current.is_some()
            && self
                .owner_dpms_installed_active
                .get(&device)
                .copied()
                .unwrap_or(true);
        let clock_key = if request.2.is_some() {
            let hardware_crtc = u32::from(output.crtc);
            let owner = self
                .platform
                .owner_ref(device)
                .ok_or_else(|| preparation_error(Stage::Allocation))?;
            let epoch =
                owner.next_clock_epoch_after(hardware_crtc, self.next_present_crtc_clock_epoch);
            if epoch.get().checked_add(1).is_none()
                || owner
                    .clock_key_for_hardware_crtc(hardware_crtc)
                    .is_some_and(|current| epoch <= current.epoch)
            {
                return Err(preparation_error(Stage::Allocation));
            }
            Some(crate::kms::owner::clock::ClockKey {
                hardware_crtc,
                epoch,
            })
        } else {
            None
        };
        let mut prepared_set = PreparedClientModesetSet {
            output: Some(output),
            output_instance_id: None,
            scanout: None,
            scene: None,
            clock_key,
            mode_blob: None,
            allocation_keys: Vec::new(),
            #[cfg(test)]
            framebuffer_handles_for_tests: Vec::new(),
        };

        let (mode_blob_id, framebuffer_id) = if let Some(mode) = request.2 {
            let output = prepared_set.output.as_ref().expect("prepared output");
            let mode_blob = drm_device
                .create_property_blob(&output.mode)
                .map_err(|_| preparation_error(Stage::Allocation))?;
            let mode_blob_raw: u64 = mode_blob.into();
            let owned_mode_blob =
                OwnedModeBlob::new(std::rc::Rc::clone(&drm_device), mode_blob_raw);
            let mode_blob_id = match u32::try_from(mode_blob_raw) {
                Ok(mode_blob_id) => mode_blob_id,
                Err(_) => {
                    drop(owned_mode_blob);
                    return Err(preparation_error(Stage::Allocation));
                }
            };
            prepared_set.mode_blob = Some(owned_mode_blob);
            #[cfg(test)]
            if std::mem::take(&mut self.client_modeset_force_allocation_failure_for_tests) {
                return Err(preparation_error(Stage::Allocation));
            }
            let mut scanout = self
                .platform
                .allocate_prepared_client_scanout_pool(
                    std::rc::Rc::clone(&drm_device),
                    output,
                    scanout_route,
                    u32::from(mode.width),
                    u32::from(mode.height),
                )
                .map_err(|_| preparation_error(Stage::Allocation))?;
            let framebuffer = scanout
                .display_pool()
                .bos
                .first()
                .and_then(|bo| bo.fb_handle)
                .map(u32::from)
                .filter(|framebuffer| *framebuffer != 0)
                .ok_or_else(|| preparation_error(Stage::Allocation))?;
            if let Some(front) = scanout.display_pool_mut().bos.first_mut() {
                front.state.mark_on_screen_after_modeset();
                scanout
                    .note_kms_modeset_installed(0)
                    .map_err(|_| preparation_error(Stage::Allocation))?;
            } else {
                return Err(preparation_error(Stage::Allocation));
            }
            #[cfg(test)]
            let framebuffer_handles_for_tests = scanout
                .display_pool()
                .bos
                .iter()
                .filter_map(|bo| bo.fb_handle.map(u32::from))
                .collect::<Vec<_>>();
            let instance_id = self
                .platform
                .allocate_output_instance_id(&output_key)
                .map_err(|_| preparation_error(Stage::Allocation))?;
            let scene = self
                .scene
                .stage_client_output_scene_state(
                    &output_key,
                    instance_id,
                    mode.width,
                    mode.height,
                    request.3,
                    request.4,
                    &scanout,
                )
                .map_err(|_| preparation_error(Stage::SceneState))?;
            let incarnation = self
                .platform
                .owner_ref(device)
                .map(|owner| owner.incarnation())
                .ok_or_else(|| preparation_error(Stage::Allocation))?;
            let service = self
                .resource_service
                .as_mut()
                .filter(|service| {
                    service.device() == device && service.incarnation() == incarnation
                })
                .ok_or_else(|| preparation_error(Stage::Allocation))?;
            let registry = self
                .drm_cleanup_registry
                .as_mut()
                .filter(|registry| {
                    registry.device_key() == device && registry.incarnation() == incarnation
                })
                .ok_or_else(|| preparation_error(Stage::Allocation))?;
            let allocation_keys = self
                .platform
                .register_prepared_client_scanout_pool(&mut scanout, service, registry)
                .map_err(|_| preparation_error(Stage::Allocation))?;
            prepared_set.output_instance_id = Some(instance_id);
            prepared_set.scanout = Some(scanout);
            prepared_set.scene = Some(scene);
            prepared_set.allocation_keys = allocation_keys;
            #[cfg(test)]
            {
                prepared_set.framebuffer_handles_for_tests = framebuffer_handles_for_tests;
            }
            (mode_blob_id, framebuffer)
        } else {
            (0, 0)
        };

        let operation = match (request.2, projection) {
            (Some(mode), Some(projection)) => ClientModesetOperation::Configure {
                width: mode.width,
                height: mode.height,
                framebuffer: framebuffer_id,
                mode_blob: mode_blob_id,
                projection,
            },
            (None, None) => ClientModesetOperation::Disable,
            _ => {
                if let (Some(service), Some(registry)) = (
                    self.resource_service.as_mut(),
                    self.drm_cleanup_registry.as_mut(),
                ) {
                    prepared_set.release(service, registry);
                }
                return Err(preparation_error(Stage::SceneState));
            }
        };
        let output = prepared_set.output.as_ref().expect("prepared output");
        let description = match build_client_modeset_description(ClientModesetDescriptionInput {
            objects: ClientModesetObjects {
                connector: u32::from(output.connector),
                crtc: u32::from(output.crtc),
                primary_plane: u32::from(output.plane),
                old_crtc_id,
            },
            properties: property_ids,
            old_active,
            operation,
        }) {
            Ok(prepared) => prepared,
            Err(_) => {
                if let (Some(service), Some(registry)) = (
                    self.resource_service.as_mut(),
                    self.drm_cleanup_registry.as_mut(),
                ) {
                    prepared_set.release(service, registry);
                }
                return Err(preparation_error(Stage::SceneState));
            }
        };
        Ok(PreparedClientModesetDescription {
            description: description.description,
            staged_projection: description.staged_projection,
            prepared_set,
        })
    }

    fn lifecycle_topology_description(
        &mut self,
        device: DrmDeviceKey,
    ) -> Result<CommitDescription, String> {
        let target = crate::kms::owner::lifecycle::dpms_target_for_level(
            self.lifecycle_coordinator.protocol_dpms_level(),
        )
        .ok_or_else(|| "lifecycle DPMS level is invalid".to_string())?;
        let projected_outputs = self
            .lifecycle_coordinator
            .device(&device)
            .ok_or_else(|| "lifecycle device has no output projection".to_string())?
            .desired()
            .dpms_targets()
            .keys()
            .cloned()
            .collect::<BTreeSet<_>>();
        let device_entry = self
            .platform
            .devices
            .iter_mut()
            .find(|entry| entry.key == device)
            .ok_or_else(|| "lifecycle device disappeared".to_string())?;
        let outputs = self
            .platform
            .outputs
            .iter()
            .filter(|output| {
                output.key.device_key == device && projected_outputs.contains(&output.key)
            })
            .collect::<Vec<_>>();
        if outputs.is_empty() {
            return Err("lifecycle device has no protocol outputs".to_string());
        }
        let members = outputs
            .iter()
            .map(
                |output| crate::kms::render::composed_commit::ComposedPlane {
                    output: &output.output,
                    // Property discovery ignores the framebuffer. The DPMS
                    // description below leaves all primary-plane properties
                    // untouched, retaining whichever buffer is currently bound.
                    framebuffer: ::drm::control::from_u32(1).expect("nonzero fixture handle"),
                },
            )
            .collect::<Vec<_>>();
        let property_ids = crate::kms::render::composed_commit::discover_composed_property_ids(
            device_entry.device.as_ref(),
            &members,
            &mut device_entry.active_property_cache,
        )
        .map_err(|error| format!("lifecycle property discovery: {error}"))?;

        // DPMS changes only CRTC power. Omitting connector and plane objects
        // retains MODE_ID, routing, and each primary plane's FB_ID/CRTC_ID.
        let mut objects = Vec::with_capacity(outputs.len());
        let mut crtc_state = Vec::with_capacity(outputs.len());
        for output in outputs {
            let crtc_id = u32::from(output.output.crtc);
            let new_active = target == crate::kms::owner::lifecycle::DpmsTarget::On;
            objects.push(crate::kms::owner::closure::SerializedObject {
                object: crtc_id,
                kind: crate::kms::owner::closure::ObjectKind::Crtc,
                old_crtc_id: None,
                props: vec![(property_ids.active, u64::from(new_active))],
            });
            crtc_state.push(crate::kms::owner::closure::CrtcPower {
                crtc_id,
                old_active: !new_active,
                new_active,
            });
        }
        Ok(CommitDescription {
            objects,
            crtc_state,
            present_consumers: Vec::new(),
            page_flip_event: false,
            property_ids,
        })
    }

    fn lifecycle_clock_readiness(
        &self,
        device: DrmDeviceKey,
        required_clock_crtcs: &[u32],
    ) -> LifecycleClockReadiness {
        let Some(owner) = self.platform.owner_ref(device) else {
            return LifecycleClockReadiness::Missing(
                required_clock_crtcs.first().copied().unwrap_or_default(),
            );
        };
        let (lifecycle, generation) = owner.clock_context();
        let mut clocks = BTreeMap::new();
        for &crtc in required_clock_crtcs {
            let Some(key) = owner.clock_key_for_hardware_crtc(crtc) else {
                return LifecycleClockReadiness::Missing(crtc);
            };
            let Some(clock) = owner.clock(key) else {
                return LifecycleClockReadiness::Missing(crtc);
            };
            if clock.lifecycle_epoch != lifecycle || clock.topology_generation != generation {
                return LifecycleClockReadiness::Inconsistent {
                    crtc,
                    key,
                    detail: "clock context does not match the current owner epoch",
                };
            }
            match (
                clock.source,
                clock.probe,
                clock.reference,
                clock.probe_outcome,
            ) {
                (
                    ClockSource::KernelSequence,
                    ProbeState::Succeeded,
                    Some(_),
                    Some(ProbeOutcome::Ready { .. }),
                ) => {
                    clocks.insert(crtc, key);
                }
                (
                    ClockSource::Unresolved,
                    ProbeState::NotStarted | ProbeState::InFlight(_),
                    _,
                    _,
                ) => {
                    return LifecycleClockReadiness::Waiting;
                }
                (ClockSource::Unresolved, ProbeState::Failed, _, Some(outcome)) => {
                    return LifecycleClockReadiness::Failed { crtc, key, outcome };
                }
                _ => {
                    return LifecycleClockReadiness::Inconsistent {
                        crtc,
                        key,
                        detail: "clock source and probe outcome disagree",
                    };
                }
            }
        }
        LifecycleClockReadiness::Ready(clocks)
    }

    fn lifecycle_log_probe_failure_once(
        &mut self,
        device: DrmDeviceKey,
        crtc: u32,
        key: ClockKey,
        outcome: ProbeOutcome,
    ) {
        let should_log = self
            .platform
            .owner_for(device)
            .and_then(|owner| owner.clock_mut(key))
            .is_some_and(|clock| {
                if clock.probe_failure_reported {
                    false
                } else {
                    clock.probe_failure_reported = true;
                    true
                }
            });
        if should_log {
            log::error!(
                "lifecycle DPMS cannot dispatch: CRTC {crtc} clock probe resolved as {outcome:?}"
            );
        }
    }

    fn lifecycle_report_never_dispatched(
        &mut self,
        device: DrmDeviceKey,
        tag: TransitionTag<IncarnationId>,
    ) {
        if let Some(conductor) = self.admission_conductors.get_mut(&device) {
            conductor
                .admission
                .cancel_topology(TopologyWork::Transition(tag));
        }
        self.lifecycle_queue_input(
            device,
            ArbiterInput::CommitOutcome {
                tag,
                outcome: LifecycleCommitOutcome::Rejected {
                    topology_latched_generation: None,
                },
            },
        );
    }

    fn lifecycle_retry_topology_without_consuming_attempt(
        &mut self,
        device: DrmDeviceKey,
        tag: TransitionTag<IncarnationId>,
    ) {
        self.lifecycle_queue_input(
            device,
            ArbiterInput::CommitProgress {
                tag,
                progress: CommitProgress::NotSubmitted,
            },
        );
    }

    fn lifecycle_dispatch_error_is_transient(error: &DispatchError<CommitResources>) -> bool {
        matches!(
            error,
            DispatchError::Slot(
                crate::kms::owner::slot::SlotError::AlreadyOccupied(_)
                    | crate::kms::owner::slot::SlotError::ValidationOutstanding(_)
                    | crate::kms::owner::slot::SlotError::ProbeOutstanding(_)
                    | crate::kms::owner::slot::SlotError::QueueOutstanding(_)
                    | crate::kms::owner::slot::SlotError::AtomicUnresolved(_),
            ) | DispatchError::ClockNotReady(_)
                | DispatchError::LegacyTransportActive
                | DispatchError::Refused {
                    cause: RefusalCause::AlreadyInFlight,
                    ..
                }
        )
    }

    fn admission_dispatch_topology(
        &mut self,
        device: DrmDeviceKey,
        token: AdmissionToken,
        decision: AdmissionDecision,
    ) -> AdmissionOutcome {
        let Admitted::Topology { work } = decision.admitted else {
            self.admission_abort(device, token);
            return AdmissionOutcome::Unsupported(decision.tier);
        };
        match work {
            TopologyWork::Transition(tag) => {
                self.admission_dispatch_lifecycle_topology(device, token, decision, tag)
            }
            TopologyWork::ClientModeset(tag) => {
                self.admission_dispatch_client_modeset(device, token, decision, tag)
            }
        }
    }

    fn admission_dispatch_lifecycle_topology(
        &mut self,
        device: DrmDeviceKey,
        token: AdmissionToken,
        decision: AdmissionDecision,
        tag: TransitionTag<IncarnationId>,
    ) -> AdmissionOutcome {
        if !self.lifecycle_tag_current(device, tag) {
            if let Some(conductor) = self.admission_conductors.get_mut(&device) {
                conductor
                    .admission
                    .cancel_topology(TopologyWork::Transition(tag));
                let _ = conductor.admission.abort(token);
            }
            self.lifecycle_queue_input(
                device,
                ArbiterInput::CommitOutcome {
                    tag,
                    outcome: LifecycleCommitOutcome::Rejected {
                        topology_latched_generation: None,
                    },
                },
            );
            return AdmissionOutcome::NothingAdmissible;
        }
        self.lifecycle_sync_owner_tag(device, tag);
        let description = match self.lifecycle_topology_description(device) {
            Ok(description) => description,
            Err(error) => {
                log::warn!("lifecycle topology description refused: {error}");
                self.admission_abort(device, token);
                self.lifecycle_queue_input(
                    device,
                    ArbiterInput::CommitOutcome {
                        tag,
                        outcome: LifecycleCommitOutcome::Rejected {
                            topology_latched_generation: None,
                        },
                    },
                );
                return AdmissionOutcome::PreparationRefused;
            }
        };
        let required_clock_crtcs = description
            .crtc_state
            .iter()
            .filter(|state| state.old_active)
            .map(|state| state.crtc_id)
            .collect::<Vec<_>>();
        match self.lifecycle_clock_readiness(device, &required_clock_crtcs) {
            LifecycleClockReadiness::Ready(_) => {}
            LifecycleClockReadiness::Waiting => {
                self.admission_abort(device, token);
                return AdmissionOutcome::NothingAdmissible;
            }
            LifecycleClockReadiness::Failed { crtc, key, outcome } => {
                self.admission_abort(device, token);
                if let ProbeOutcome::Unknown(_) = outcome {
                    self.lifecycle_report_completion_loss(device);
                } else {
                    self.lifecycle_log_probe_failure_once(device, crtc, key, outcome);
                    self.lifecycle_report_never_dispatched(device, tag);
                }
                return AdmissionOutcome::BeginRefused;
            }
            LifecycleClockReadiness::Missing(crtc) => {
                log::error!(
                    "lifecycle topology for {device:?} names served CRTC {crtc} without a clock record"
                );
                self.admission_abort(device, token);
                self.lifecycle_report_never_dispatched(device, tag);
                return AdmissionOutcome::BeginRefused;
            }
            LifecycleClockReadiness::Inconsistent { crtc, key, detail } => {
                log::error!(
                    "lifecycle topology for {device:?} has inconsistent CRTC {crtc} clock {key:?}: {detail}"
                );
                self.admission_abort(device, token);
                self.lifecycle_report_never_dispatched(device, tag);
                return AdmissionOutcome::BeginRefused;
            }
        }
        let dpms_active = self.lifecycle_coordinator.protocol_dpms_level() == 0;
        let validation_result = self.platform.owner_for(device).map(|owner| {
            owner.begin_validation_with_options(
                &description,
                crate::kms::executor::HostCallClass::SeatActiveValidation,
                true,
            )
        });
        let validation_commit = match validation_result {
            Some(Ok(commit)) => commit,
            Some(Err(error)) => {
                self.admission_abort(device, token);
                if Self::lifecycle_dispatch_error_is_transient(&error) {
                    return AdmissionOutcome::BeginRefused;
                }
                log::error!(
                    "lifecycle validation for {device:?} was refused before dispatch: {error}"
                );
                self.lifecycle_report_never_dispatched(device, tag);
                return AdmissionOutcome::BeginRefused;
            }
            None => {
                log::error!(
                    "lifecycle validation for {device:?} was never dispatched: owner disappeared"
                );
                self.admission_abort(device, token);
                self.lifecycle_report_never_dispatched(device, tag);
                return AdmissionOutcome::BeginRefused;
            }
        };
        if let Some(driver) = self.lifecycle_drivers.get_mut(&device) {
            driver.pending_topology_validations.insert(
                validation_commit,
                PendingTopologyValidation {
                    tag,
                    decision,
                    token: Some(token),
                    description,
                    dpms_active,
                    sent: false,
                    cancelled: false,
                },
            );
        }

        #[cfg(test)]
        self.lifecycle_apply_topology_hook(
            device,
            LifecycleTopologyTestHook::SupersedeBeforeTestOnly,
        );
        if !self.lifecycle_tag_current(device, tag) {
            #[cfg(test)]
            if let Some(driver) = self.lifecycle_drivers.get_mut(&device) {
                driver.stale_before_test_only = driver.stale_before_test_only.saturating_add(1);
            }
            if let Some(owner) = self.platform.owner_for(device) {
                let _ = owner.abandon_validation(validation_commit);
            }
            self.lifecycle_abort_pending_validation(device, validation_commit, true);
            self.lifecycle_queue_input(
                device,
                ArbiterInput::CommitOutcome {
                    tag,
                    outcome: LifecycleCommitOutcome::Rejected {
                        topology_latched_generation: None,
                    },
                },
            );
            return AdmissionOutcome::NothingAdmissible;
        }

        if let Some(driver) = self.lifecycle_drivers.get_mut(&device)
            && let Some(pending) = driver
                .pending_topology_validations
                .get_mut(&validation_commit)
        {
            pending.sent = true;
        }
        #[cfg(test)]
        if let Some(driver) = self.lifecycle_drivers.get_mut(&device) {
            driver.validation_sends.push(tag);
        }
        let send_result = self
            .platform
            .devices
            .iter_mut()
            .find(|entry| entry.key == device)
            .and_then(|entry| {
                let owner = entry.owner.as_mut()?;
                let executor = entry.executor.as_mut()?;
                Some(owner.send_validation_on(executor))
            });
        match send_result {
            Some(Ok(_events)) => {
                self.lifecycle_queue_input(
                    device,
                    ArbiterInput::CommitProgress {
                        tag,
                        progress: CommitProgress::Submitting,
                    },
                );
                AdmissionOutcome::NothingAdmissible
            }
            Some(Err(error @ DispatchError::Refused { .. })) => {
                let cause = match &error {
                    DispatchError::Refused { cause, .. } => *cause,
                    _ => unreachable!(),
                };
                self.lifecycle_abort_pending_validation(device, validation_commit, true);
                if cause == RefusalCause::AlreadyInFlight {
                    return AdmissionOutcome::SendRefused(cause);
                }
                log::error!(
                    "lifecycle validation for {device:?} was refused before dispatch: {error}"
                );
                self.lifecycle_dispose_topology_result(
                    device,
                    validation_commit,
                    tag,
                    TerminalState::FailedBeforeSubmit(FailureCause::NeverDispatched(cause)),
                );
                AdmissionOutcome::SendRefused(cause)
            }
            Some(Err(error)) => {
                if let Some(owner) = self.platform.owner_for(device) {
                    let _ = owner.abandon_validation(validation_commit);
                }
                self.lifecycle_abort_pending_validation(device, validation_commit, true);
                log::error!("lifecycle validation for {device:?} failed before dispatch: {error}");
                self.lifecycle_dispose_topology_result(
                    device,
                    validation_commit,
                    tag,
                    TerminalState::FailedBeforeSubmit(FailureCause::NeverDispatched(
                        RefusalCause::OwnerInternalError,
                    )),
                );
                AdmissionOutcome::BeginRefused
            }
            None => {
                if let Some(owner) = self.platform.owner_for(device) {
                    let _ = owner.abandon_validation(validation_commit);
                }
                self.lifecycle_abort_pending_validation(device, validation_commit, true);
                log::error!(
                    "lifecycle validation for {device:?} was never dispatched: owner or executor disappeared"
                );
                self.lifecycle_dispose_topology_result(
                    device,
                    validation_commit,
                    tag,
                    TerminalState::FailedBeforeSubmit(FailureCause::NeverDispatched(
                        RefusalCause::OwnerInternalError,
                    )),
                );
                AdmissionOutcome::BeginRefused
            }
        }
    }

    fn admission_dispatch_client_modeset(
        &mut self,
        device: DrmDeviceKey,
        token: AdmissionToken,
        decision: AdmissionDecision,
        tag: ClientModesetTag<IncarnationId>,
    ) -> AdmissionOutcome {
        let Some(arbiter) = self.lifecycle_coordinator.device(&device) else {
            self.admission_abort(device, token);
            return AdmissionOutcome::BeginRefused;
        };
        if arbiter.state() != crate::kms::owner::lifecycle::DeviceLifecycleState::Ready {
            self.admission_abort(device, token);
            return AdmissionOutcome::NothingAdmissible;
        }
        if !self.client_modeset_tag_current(device, tag)
            || self
                .lifecycle_coordinator
                .device(&device)
                .is_none_or(|arbiter| arbiter.transition().is_some())
        {
            self.admission_abort(device, token);
            self.lifecycle_complete_queued_client_modeset(
                device,
                tag,
                self.lifecycle_superseding_kind(device),
            );
            return AdmissionOutcome::NothingAdmissible;
        }
        if self
            .scanout_m2
            .client_modeset_unflip_holds
            .iter()
            .any(|hold| hold.modeset_device == device && hold.tag == tag && !hold.unflip_terminal)
        {
            self.admission_abort(device, token);
            return AdmissionOutcome::NothingAdmissible;
        }

        let prepared = self
            .lifecycle_drivers
            .get(&device)
            .and_then(|driver| driver.client_modeset.as_ref())
            .filter(|slot| slot.tag == tag)
            .and_then(|slot| slot.prepared.as_ref())
            .map(|prepared| {
                Ok((
                    prepared.description.clone(),
                    prepared.staged_projection.clone(),
                ))
            })
            .unwrap_or_else(|| {
                self.lifecycle_prepare_client_modeset(device, tag)
                    .map(|prepared| {
                        let result = (
                            prepared.description.clone(),
                            prepared.staged_projection.clone(),
                        );
                        if let Some(slot) = self
                            .lifecycle_drivers
                            .get_mut(&device)
                            .and_then(|driver| driver.client_modeset.as_mut())
                            .filter(|slot| slot.tag == tag)
                        {
                            slot.prepared = Some(prepared);
                        }
                        result
                    })
            });
        let (description, staged_projection) = match prepared {
            Ok(prepared) => prepared,
            Err(error) => {
                self.admission_abort(device, token);
                self.lifecycle_complete_client_modeset_without_dispatch(device, tag, Err(error));
                return AdmissionOutcome::PreparationRefused;
            }
        };
        if self.lifecycle_client_modeset_requires_direct_unflip(device, tag) {
            self.lifecycle_park_client_modeset_for_unflip(device, token, tag);
            return AdmissionOutcome::NothingAdmissible;
        }
        let required_clock_crtcs = description
            .crtc_state
            .iter()
            .filter(|state| state.old_active)
            .map(|state| state.crtc_id)
            .collect::<Vec<_>>();
        match self.lifecycle_clock_readiness(device, &required_clock_crtcs) {
            LifecycleClockReadiness::Ready(_) => {}
            LifecycleClockReadiness::Waiting => {
                self.admission_abort(device, token);
                return AdmissionOutcome::NothingAdmissible;
            }
            LifecycleClockReadiness::Failed { .. }
            | LifecycleClockReadiness::Missing(_)
            | LifecycleClockReadiness::Inconsistent { .. } => {
                self.admission_abort(device, token);
                self.lifecycle_complete_client_modeset_without_dispatch(
                    device,
                    tag,
                    Err(std::io::Error::new(
                        std::io::ErrorKind::Interrupted,
                        ClientModesetFailure::OwnerRefused(OwnerRefusal::ClockNotReady),
                    )),
                );
                return AdmissionOutcome::BeginRefused;
            }
        }
        let validation_commit = match self.platform.owner_for(device).map(|owner| {
            owner.begin_validation_with_options(
                &description,
                crate::kms::executor::HostCallClass::SeatActiveValidation,
                true,
            )
        }) {
            Some(Ok(commit)) => commit,
            Some(Err(error)) => {
                self.admission_abort(device, token);
                if Self::lifecycle_dispatch_error_is_transient(&error) {
                    return AdmissionOutcome::BeginRefused;
                }
                self.lifecycle_complete_client_modeset_without_dispatch(
                    device,
                    tag,
                    Err(std::io::Error::other(format!(
                        "Owner modeset validation refused: {error}"
                    ))),
                );
                return AdmissionOutcome::BeginRefused;
            }
            None => {
                self.admission_abort(device, token);
                self.lifecycle_complete_client_modeset_without_dispatch(
                    device,
                    tag,
                    Err(std::io::Error::other(
                        "Owner disappeared before modeset validation",
                    )),
                );
                return AdmissionOutcome::BeginRefused;
            }
        };
        if let Some(driver) = self.lifecycle_drivers.get_mut(&device) {
            if let Some(slot) = driver.client_modeset.as_mut()
                && slot.tag == tag
            {
                slot.phase = ClientModesetPhase::Validating;
            }
            driver.pending_client_modeset_validations.insert(
                validation_commit,
                PendingClientModesetValidation {
                    tag,
                    decision,
                    token: Some(token),
                    description,
                    staged_projection: staged_projection.clone(),
                    sent: false,
                    cancelled: false,
                },
            );
        }
        #[cfg(test)]
        if std::mem::take(&mut self.client_modeset_stale_before_validation_for_tests)
            && let Some(owner) = self.platform.owner_for(device)
        {
            let next_generation = owner
                .topology_generation()
                .checked_add(1)
                .expect("test topology generation does not overflow");
            let _ = owner.invalidate_topology(next_generation);
        }
        if !self.client_modeset_tag_current(device, tag)
            || !self.client_modeset_projection_current(device, staged_projection.as_ref())
        {
            if let Some(owner) = self.platform.owner_for(device) {
                let _ = owner.abandon_validation(validation_commit);
            }
            self.lifecycle_finish_client_modeset_validation_cancelled(
                device,
                validation_commit,
                tag,
            );
            return AdmissionOutcome::NothingAdmissible;
        }
        if let Some(pending) = self.lifecycle_drivers.get_mut(&device).and_then(|driver| {
            driver
                .pending_client_modeset_validations
                .get_mut(&validation_commit)
        }) {
            pending.sent = true;
        }
        let send_result = self
            .platform
            .devices
            .iter_mut()
            .find(|entry| entry.key == device)
            .and_then(|entry| {
                let owner = entry.owner.as_mut()?;
                let executor = entry.executor.as_mut()?;
                Some(owner.send_validation_on(executor))
            });
        match send_result {
            Some(Ok(_events)) => {
                // Only a validation the owner actually sent to the executor
                // counts; a refusal (for example AlreadyInFlight) re-queues.
                #[cfg(test)]
                if let Some(driver) = self.lifecycle_drivers.get_mut(&device) {
                    driver.client_validation_sends.push(tag);
                }
                AdmissionOutcome::NothingAdmissible
            }
            Some(Err(error @ DispatchError::Refused { .. })) => {
                let cause = match error {
                    DispatchError::Refused { cause, .. } => cause,
                    _ => unreachable!(),
                };
                if cause == RefusalCause::AlreadyInFlight {
                    self.lifecycle_abort_client_modeset_validation(device, validation_commit, true);
                    if let Some(slot) = self
                        .lifecycle_drivers
                        .get_mut(&device)
                        .and_then(|driver| driver.client_modeset.as_mut())
                        && slot.tag == tag
                    {
                        slot.phase = ClientModesetPhase::Queued;
                    }
                    AdmissionOutcome::SendRefused(cause)
                } else {
                    self.lifecycle_abort_client_modeset_validation(device, validation_commit, true);
                    self.lifecycle_complete_client_modeset_without_dispatch(
                        device,
                        tag,
                        Err(std::io::Error::other(format!(
                            "Owner modeset validation refused: {cause:?}"
                        ))),
                    );
                    AdmissionOutcome::SendRefused(cause)
                }
            }
            Some(Err(error)) => {
                self.lifecycle_abort_client_modeset_validation(device, validation_commit, true);
                self.lifecycle_complete_client_modeset_without_dispatch(
                    device,
                    tag,
                    Err(std::io::Error::other(format!(
                        "Owner modeset validation failed: {error}"
                    ))),
                );
                AdmissionOutcome::BeginRefused
            }
            None => {
                self.lifecycle_abort_client_modeset_validation(device, validation_commit, true);
                self.lifecycle_complete_client_modeset_without_dispatch(
                    device,
                    tag,
                    Err(std::io::Error::other(
                        "Owner executor disappeared before validation",
                    )),
                );
                AdmissionOutcome::BeginRefused
            }
        }
    }

    fn lifecycle_abort_client_modeset_validation(
        &mut self,
        device: DrmDeviceKey,
        commit: CommitId,
        abort_token: bool,
    ) {
        let pending = self
            .lifecycle_drivers
            .get_mut(&device)
            .and_then(|driver| driver.pending_client_modeset_validations.remove(&commit));
        if let Some(pending) = pending
            && abort_token
            && let (Some(token), Some(conductor)) =
                (pending.token, self.admission_conductors.get_mut(&device))
        {
            let _ = conductor.admission.abort(token);
        }
    }

    fn lifecycle_complete_client_modeset_without_dispatch(
        &mut self,
        device: DrmDeviceKey,
        tag: ClientModesetTag<IncarnationId>,
        result: std::io::Result<bool>,
    ) {
        self.lifecycle_complete_client_modeset_without_dispatch_with_readiness(
            device, tag, result, true,
        );
    }

    fn lifecycle_complete_client_modeset_without_dispatch_with_readiness(
        &mut self,
        device: DrmDeviceKey,
        tag: ClientModesetTag<IncarnationId>,
        result: std::io::Result<bool>,
        close_on_completion_unknown: bool,
    ) {
        if let Some(conductor) = self.admission_conductors.get_mut(&device) {
            conductor
                .admission
                .cancel_topology(TopologyWork::ClientModeset(tag));
        }
        let slot = self.lifecycle_take_client_modeset_slot(device, tag);
        if let Some(slot) = slot {
            let token = slot.token;
            let failure = result
                .as_ref()
                .err()
                .and_then(client_modeset_failure_from_error);
            let closes_readiness = matches!(
                failure,
                Some(ClientModesetFailure::OwnerRefused(
                    OwnerRefusal::ReadinessClosed
                ))
            ) || (close_on_completion_unknown
                && failure == Some(ClientModesetFailure::CompletionUnknown))
                || matches!(
                    failure,
                    Some(ClientModesetFailure::Preparation(PreparationStage::TestOnly {
                        errno,
                    })) if !test_only_errno_keeps_readiness(errno)
                );
            if closes_readiness {
                self.lifecycle_close_client_modeset_readiness(device, tag);
            }
            let result = client_modeset_error_for_slot(device, &slot, result);
            self.lifecycle_release_client_modeset_slot(slot);
            self.complete_owner_client_modeset(token, result);
        }
        self.lifecycle_end_client_modeset_direct_hold(device, tag, false);
    }

    fn lifecycle_close_client_modeset_readiness(
        &mut self,
        device: DrmDeviceKey,
        tag: ClientModesetTag<IncarnationId>,
    ) {
        match self
            .lifecycle_coordinator
            .client_modeset_close_readiness(&device, &tag)
        {
            Ok(true) => {
                if let Some(conductor) = self.admission_conductors.get_mut(&device) {
                    conductor.lifecycle_admission_closed = true;
                }
            }
            Ok(false) => {}
            Err(error) => {
                log::error!("client modeset readiness could not close for {device}: {error:?}")
            }
        }
    }

    fn lifecycle_finish_client_modeset_validation_cancelled(
        &mut self,
        device: DrmDeviceKey,
        commit: CommitId,
        tag: ClientModesetTag<IncarnationId>,
    ) {
        self.lifecycle_abort_client_modeset_validation(device, commit, true);
        self.lifecycle_complete_queued_client_modeset(
            device,
            tag,
            self.lifecycle_superseding_kind(device),
        );
    }

    fn lifecycle_client_modeset_requires_direct_unflip(
        &self,
        device: DrmDeviceKey,
        tag: ClientModesetTag<IncarnationId>,
    ) -> bool {
        let Some(slot) = self
            .lifecycle_drivers
            .get(&device)
            .and_then(|driver| driver.client_modeset.as_ref())
            .filter(|slot| slot.tag == tag)
        else {
            return false;
        };
        if !self.scanout_m2.active() {
            return false;
        }
        let direct_device = self.direct_scanout_device_for_unflip();
        let output_key = crate::kms::backend::OutputKey::new(device, slot.connector.clone());
        let prepared_output = slot
            .prepared
            .as_ref()
            .and_then(|prepared| prepared.prepared_set.output.as_ref());
        let mut projected = Vec::with_capacity(self.platform.outputs.len() + 1);
        let mut saw_target = false;
        for output in &self.platform.outputs {
            if output.key != output_key {
                projected.push((
                    output.key.device_key,
                    output.x,
                    output.y,
                    output.width,
                    output.height,
                    output.output.picked.clone(),
                ));
                continue;
            }
            saw_target = true;
            if let Some(mode) = slot.mode {
                let Some(prepared_output) = prepared_output else {
                    return true;
                };
                projected.push((
                    device,
                    slot.x,
                    slot.y,
                    mode.width,
                    mode.height,
                    prepared_output.picked.clone(),
                ));
            }
        }
        if let Some(mode) = slot.mode
            && !saw_target
        {
            let Some(prepared_output) = prepared_output else {
                return true;
            };
            projected.push((
                device,
                slot.x,
                slot.y,
                mode.width,
                mode.height,
                prepared_output.picked.clone(),
            ));
        }

        let topology_direct_eligible = self
            .platform
            .primary_device()
            .and_then(|primary| {
                let first = projected.first()?;
                Some(
                    first.0 == primary.key
                        && projected.iter().all(|output| {
                            output.0 == primary.key
                                && crate::kms::render::backend::effective_refresh_matches(
                                    &first.5, &output.5,
                                )
                        }),
                )
            })
            .unwrap_or(false);
        let projected_extent = crate::kms::render::platform::recompute_fb_extent_from(
            &projected
                .iter()
                .map(|output| (output.1, output.2, output.3, output.4))
                .collect::<Vec<_>>(),
        );
        let root_extent_changes = projected_extent != (self.platform.fb_w, self.platform.fb_h);

        direct_device == Some(device) || !topology_direct_eligible || root_extent_changes
    }

    fn lifecycle_park_client_modeset_for_unflip(
        &mut self,
        device: DrmDeviceKey,
        token: AdmissionToken,
        tag: ClientModesetTag<IncarnationId>,
    ) {
        let unflip_device = self.direct_scanout_device_for_unflip();
        let Some(unflip_device) = unflip_device else {
            self.admission_abort(device, token);
            self.lifecycle_complete_client_modeset_without_dispatch(
                device,
                tag,
                Err(std::io::Error::other(ClientModesetFailure::Preparation(
                    PreparationStage::Unflip,
                ))),
            );
            return;
        };
        if !self.admission_is_active(unflip_device) {
            self.admission_abort(device, token);
            self.lifecycle_complete_client_modeset_without_dispatch(
                device,
                tag,
                Err(std::io::Error::other(ClientModesetFailure::Preparation(
                    PreparationStage::Unflip,
                ))),
            );
            return;
        }
        if !self
            .scanout_m2
            .client_modeset_unflip_holds
            .iter()
            .any(|hold| hold.modeset_device == device && hold.tag == tag)
        {
            let in_flight = self
                .scanout_m2
                .owner_unflip_return
                .as_ref()
                .filter(|record| record.commit.device == unflip_device)
                .map(|record| record.commit);
            self.scanout_m2
                .client_modeset_unflip_holds
                .push(ClientModesetUnflipHold {
                    modeset_device: device,
                    tag,
                    unflip_device,
                    unflip_commit: in_flight,
                    unflip_terminal: false,
                    request_ended: false,
                });
        }

        self.admission_abort(device, token);
        if unflip_device == device
            && let Some(conductor) = self.admission_conductors.get_mut(&device)
        {
            conductor
                .admission
                .cancel_topology(TopologyWork::ClientModeset(tag));
        }
        self.request_direct_unflip("client_modeset_direct_ineligible");
        let queued = self
            .admission_conductors
            .get(&unflip_device)
            .is_some_and(|conductor| conductor.admission.unflip().is_some());
        let returned = self
            .scanout_m2
            .owner_unflip_return
            .as_ref()
            .is_some_and(|record| record.commit.device == unflip_device);
        if !queued && !returned {
            self.lifecycle_client_modeset_unflip_dispatch_failed(unflip_device, None);
            return;
        }
        if self.direct_unflip_shadow_ready() {
            let outcome = self.admission_wake(unflip_device, false);
            if matches!(
                outcome,
                AdmissionOutcome::PreparationRefused
                    | AdmissionOutcome::BeginRefused
                    | AdmissionOutcome::TransportClosed
                    | AdmissionOutcome::Unsupported(_)
            ) {
                self.lifecycle_client_modeset_unflip_dispatch_failed(unflip_device, None);
            }
        }
    }

    fn lifecycle_end_client_modeset_direct_hold(
        &mut self,
        device: DrmDeviceKey,
        tag: ClientModesetTag<IncarnationId>,
        promoted: bool,
    ) {
        let Some(index) = self
            .scanout_m2
            .client_modeset_unflip_holds
            .iter()
            .position(|hold| hold.modeset_device == device && hold.tag == tag)
        else {
            return;
        };
        if self.scanout_m2.client_modeset_unflip_holds[index].unflip_terminal {
            self.scanout_m2.client_modeset_unflip_holds.remove(index);
            if promoted {
                self.scanout_m2.reset_eligible_root_probation();
            }
        } else {
            self.scanout_m2.client_modeset_unflip_holds[index].request_ended = true;
        }
    }

    pub(crate) fn lifecycle_client_modeset_unflip_retired(
        &mut self,
        unflip_device: DrmDeviceKey,
        commit: crate::kms::render::resources::CommitKey,
    ) {
        let holds = &mut self.scanout_m2.client_modeset_unflip_holds;
        for hold in holds.iter_mut().filter(|hold| {
            hold.unflip_device == unflip_device && hold.unflip_commit == Some(commit)
        }) {
            hold.unflip_terminal = true;
        }
        let ready = holds
            .iter()
            .filter(|hold| {
                hold.unflip_device == unflip_device
                    && hold.unflip_commit == Some(commit)
                    && hold.unflip_terminal
            })
            .map(|hold| (hold.modeset_device, hold.tag, hold.request_ended))
            .collect::<Vec<_>>();
        let mut wake = Vec::new();
        for (modeset_device, tag, request_ended) in ready {
            let live = !request_ended
                && self.client_modeset_tag_current(modeset_device, tag)
                && self
                    .lifecycle_drivers
                    .get(&modeset_device)
                    .and_then(|driver| driver.client_modeset.as_ref())
                    .is_some_and(|slot| slot.tag == tag);
            if live {
                if let Some(conductor) = self.admission_conductors.get_mut(&modeset_device) {
                    let _ = conductor
                        .admission
                        .request_topology(TopologyWork::ClientModeset(tag));
                    if modeset_device != unflip_device {
                        wake.push(modeset_device);
                    }
                }
            } else {
                self.lifecycle_end_client_modeset_direct_hold(modeset_device, tag, false);
            }
        }
        for device in wake {
            let _ = self.admission_wake(device, false);
        }
    }

    pub(crate) fn lifecycle_client_modeset_unflip_terminal(
        &mut self,
        unflip_device: DrmDeviceKey,
        commit: crate::kms::render::resources::CommitKey,
        terminal: TerminalState,
    ) {
        if !self
            .scanout_m2
            .client_modeset_unflip_holds
            .iter()
            .any(|hold| hold.unflip_device == unflip_device && hold.unflip_commit == Some(commit))
        {
            return;
        }
        match terminal {
            TerminalState::Completed => return,
            TerminalState::FailedBeforeSubmit(_) | TerminalState::CompletionUnknown(_) => {}
        }
        if matches!(terminal, TerminalState::CompletionUnknown(_)) {
            let affected = self
                .scanout_m2
                .client_modeset_unflip_holds
                .iter_mut()
                .filter(|hold| {
                    hold.unflip_device == unflip_device && hold.unflip_commit == Some(commit)
                })
                .map(|hold| {
                    hold.unflip_terminal = true;
                    (hold.modeset_device, hold.tag, hold.request_ended)
                })
                .collect::<Vec<_>>();
            for (modeset_device, tag, request_ended) in affected {
                if !request_ended
                    && self
                        .lifecycle_drivers
                        .get(&modeset_device)
                        .and_then(|driver| driver.client_modeset.as_ref())
                        .is_some_and(|slot| slot.tag == tag)
                {
                    self.lifecycle_complete_client_modeset_without_dispatch_with_readiness(
                        modeset_device,
                        tag,
                        Err(std::io::Error::other(
                            ClientModesetFailure::CompletionUnknown,
                        )),
                        false,
                    );
                } else {
                    self.lifecycle_end_client_modeset_direct_hold(modeset_device, tag, false);
                }
            }
            self.lifecycle_report_completion_loss(unflip_device);
        } else {
            self.lifecycle_client_modeset_unflip_dispatch_failed(unflip_device, Some(commit));
        }
    }

    fn lifecycle_client_modeset_unflip_dispatch_failed(
        &mut self,
        unflip_device: DrmDeviceKey,
        commit: Option<crate::kms::render::resources::CommitKey>,
    ) {
        let affected = self
            .scanout_m2
            .client_modeset_unflip_holds
            .iter_mut()
            .filter(|hold| {
                hold.unflip_device == unflip_device
                    && commit.map_or(hold.unflip_commit.is_none(), |commit| {
                        hold.unflip_commit == Some(commit)
                    })
            })
            .map(|hold| {
                hold.unflip_terminal = true;
                (hold.modeset_device, hold.tag, hold.request_ended)
            })
            .collect::<Vec<_>>();
        for (modeset_device, tag, request_ended) in affected {
            if !request_ended
                && self
                    .lifecycle_drivers
                    .get(&modeset_device)
                    .and_then(|driver| driver.client_modeset.as_ref())
                    .is_some_and(|slot| slot.tag == tag)
            {
                self.lifecycle_complete_client_modeset_without_dispatch(
                    modeset_device,
                    tag,
                    Err(std::io::Error::other(ClientModesetFailure::Preparation(
                        PreparationStage::Unflip,
                    ))),
                );
            } else {
                self.lifecycle_end_client_modeset_direct_hold(modeset_device, tag, false);
            }
        }
    }

    fn lifecycle_abort_pending_validation(
        &mut self,
        device: DrmDeviceKey,
        commit: CommitId,
        abort_token: bool,
    ) {
        let pending = self
            .lifecycle_drivers
            .get_mut(&device)
            .and_then(|driver| driver.pending_topology_validations.remove(&commit));
        if let Some(pending) = pending
            && abort_token
            && let (Some(token), Some(conductor)) =
                (pending.token, self.admission_conductors.get_mut(&device))
        {
            let _ = conductor.admission.abort(token);
        }
    }

    fn lifecycle_finish_client_modeset_validation(
        &mut self,
        device: DrmDeviceKey,
        validation_commit: CommitId,
        outcome: crate::kms::owner::device::ValidationOutcome,
    ) {
        let Some(mut pending) = self.lifecycle_drivers.get_mut(&device).and_then(|driver| {
            driver
                .pending_client_modeset_validations
                .remove(&validation_commit)
        }) else {
            return;
        };
        let tag = pending.tag;
        let stale = !self.client_modeset_tag_current(device, tag)
            || !self.client_modeset_projection_current(device, pending.staged_projection.as_ref());
        if pending.cancelled || stale {
            if let Some(owner) = self.platform.owner_for(device) {
                let _ = owner.abandon_validation(validation_commit);
            }
            if let Some(token) = pending.token.take()
                && let Some(conductor) = self.admission_conductors.get_mut(&device)
            {
                let _ = conductor.admission.abort(token);
            }
            if stale && !pending.cancelled {
                self.lifecycle_complete_client_modeset_without_dispatch(
                    device,
                    tag,
                    Err(std::io::Error::other(ClientModesetFailure::Stale)),
                );
            } else {
                self.lifecycle_complete_queued_client_modeset(
                    device,
                    tag,
                    self.lifecycle_superseding_kind(device),
                );
            }
            return;
        }
        match outcome {
            crate::kms::owner::device::ValidationOutcome::Passed => {
                self.lifecycle_submit_validated_client_modeset(device, validation_commit, pending);
            }
            crate::kms::owner::device::ValidationOutcome::Rejected { errno } => {
                if let Some(owner) = self.platform.owner_for(device) {
                    let _ = owner.abandon_validation(validation_commit);
                }
                if let Some(token) = pending.token.take()
                    && let Some(conductor) = self.admission_conductors.get_mut(&device)
                {
                    let _ = conductor.admission.abort(token);
                }
                self.lifecycle_complete_client_modeset_without_dispatch(
                    device,
                    tag,
                    Err(std::io::Error::other(ClientModesetFailure::Preparation(
                        PreparationStage::TestOnly { errno },
                    ))),
                );
            }
            crate::kms::owner::device::ValidationOutcome::Abandoned(_) => {
                if let Some(owner) = self.platform.owner_for(device) {
                    let _ = owner.abandon_validation(validation_commit);
                }
                if let Some(token) = pending.token.take()
                    && let Some(conductor) = self.admission_conductors.get_mut(&device)
                {
                    let _ = conductor.admission.abort(token);
                }
                self.lifecycle_complete_client_modeset_without_dispatch(
                    device,
                    tag,
                    Err(std::io::Error::other(
                        ClientModesetFailure::CompletionUnknown,
                    )),
                );
            }
        }
    }

    fn lifecycle_submit_validated_client_modeset(
        &mut self,
        device: DrmDeviceKey,
        validation_commit: CommitId,
        mut pending: PendingClientModesetValidation,
    ) {
        let tag = pending.tag;
        if !self.client_modeset_tag_current(device, tag)
            || !self.client_modeset_projection_current(device, pending.staged_projection.as_ref())
            || self
                .lifecycle_coordinator
                .device(&device)
                .is_none_or(|arbiter| arbiter.transition().is_some())
        {
            if let Some(owner) = self.platform.owner_for(device) {
                let _ = owner.abandon_validation(validation_commit);
            }
            if let Some(token) = pending.token.take()
                && let Some(conductor) = self.admission_conductors.get_mut(&device)
            {
                let _ = conductor.admission.abort(token);
            }
            self.lifecycle_complete_queued_client_modeset(
                device,
                tag,
                self.lifecycle_superseding_kind(device),
            );
            return;
        }
        let Some(token) = pending.token.take() else {
            return;
        };
        let expected_crtcs = pending
            .description
            .crtc_state
            .iter()
            .filter(|state| state.old_active || state.new_active)
            .map(|state| state.crtc_id)
            .collect::<Vec<_>>();
        let owner_commit_power_changes = pending
            .description
            .crtc_state
            .iter()
            .map(|state| OwnerCrtcPowerChange {
                incarnation: tag.incarnation,
                crtc: state.crtc_id,
                new_active: state.new_active,
                expected_completion: state.old_active || state.new_active,
            })
            .collect::<Vec<_>>();
        let connector = self
            .lifecycle_drivers
            .get(&device)
            .and_then(|driver| driver.client_modeset.as_ref())
            .filter(|slot| slot.tag == tag)
            .map(|slot| slot.connector.clone());
        let displaced_pool = match (
            connector.as_deref(),
            pending
                .description
                .crtc_state
                .first()
                .map(|state| state.crtc_id),
        ) {
            (Some(connector), Some(crtc)) => {
                self.client_modeset_displaced_pool(device, connector, crtc)
            }
            _ => Err(ResourceError::InvalidState),
        };
        let displaced_pool = match displaced_pool {
            Ok(displaced_pool) => displaced_pool,
            Err(error) => {
                if let Some(owner) = self.platform.owner_for(device) {
                    let _ = owner.abandon_validation(validation_commit);
                }
                if let Some(conductor) = self.admission_conductors.get_mut(&device) {
                    let _ = conductor.admission.abort(token);
                }
                self.lifecycle_complete_client_modeset_without_dispatch(
                    device,
                    tag,
                    Err(std::io::Error::other(format!(
                        "Owner modeset displacement resources were not ready: {error}"
                    ))),
                );
                return;
            }
        };
        let required_clock_crtcs = pending
            .description
            .crtc_state
            .iter()
            .filter(|state| state.old_active)
            .map(|state| state.crtc_id)
            .collect::<Vec<_>>();
        let clocks = match self.lifecycle_clock_readiness(device, &required_clock_crtcs) {
            LifecycleClockReadiness::Ready(clocks) => clocks,
            LifecycleClockReadiness::Waiting => {
                if let Some(owner) = self.platform.owner_for(device) {
                    let _ = owner.abandon_validation(validation_commit);
                }
                if let Some(conductor) = self.admission_conductors.get_mut(&device) {
                    let _ = conductor.admission.abort(token);
                }
                if let Some(slot) = self
                    .lifecycle_drivers
                    .get_mut(&device)
                    .and_then(|driver| driver.client_modeset.as_mut())
                    && slot.tag == tag
                {
                    slot.phase = ClientModesetPhase::Queued;
                }
                return;
            }
            LifecycleClockReadiness::Failed { .. }
            | LifecycleClockReadiness::Missing(_)
            | LifecycleClockReadiness::Inconsistent { .. } => {
                if let Some(owner) = self.platform.owner_for(device) {
                    let _ = owner.abandon_validation(validation_commit);
                }
                if let Some(conductor) = self.admission_conductors.get_mut(&device) {
                    let _ = conductor.admission.abort(token);
                }
                self.lifecycle_complete_client_modeset_without_dispatch(
                    device,
                    tag,
                    Err(std::io::Error::new(
                        std::io::ErrorKind::Interrupted,
                        ClientModesetFailure::OwnerRefused(OwnerRefusal::ClockNotReady),
                    )),
                );
                return;
            }
        };
        let mode_periods = expected_crtcs
            .iter()
            .map(|&crtc| (crtc, None))
            .collect::<BTreeMap<_, _>>();
        let completion_context = crate::kms::owner::completion::CompletionContext {
            class: crate::kms::owner::completion::CompletionClass::LifecycleInstallRestore,
            host_class: crate::kms::executor::HostCallClass::SeatActiveNonblock,
            allow_modeset: true,
            clocks,
            mode_periods,
            lifecycle_observed_max: None,
        };
        let begin_result = self.platform.owner_for(device).map(|owner| {
            owner.begin_validated_with_context(
                &pending.description,
                crate::kms::owner::ledger::Submitted::new(Vec::new(), Vec::new()),
                completion_context,
            )
        });
        let commit = match begin_result {
            Some(Ok((commit, _events))) => commit,
            Some(Err(error)) => {
                if let Some(owner) = self.platform.owner_for(device) {
                    let _ = owner.abandon_validation(validation_commit);
                }
                if let Some(conductor) = self.admission_conductors.get_mut(&device) {
                    let _ = conductor.admission.abort(token);
                }
                self.lifecycle_complete_client_modeset_without_dispatch(
                    device,
                    tag,
                    Err(std::io::Error::other(format!(
                        "Owner modeset dispatch refused: {error}"
                    ))),
                );
                return;
            }
            None => {
                if let Some(owner) = self.platform.owner_for(device) {
                    let _ = owner.abandon_validation(validation_commit);
                }
                if let Some(conductor) = self.admission_conductors.get_mut(&device) {
                    let _ = conductor.admission.abort(token);
                }
                self.lifecycle_complete_client_modeset_without_dispatch(
                    device,
                    tag,
                    Err(std::io::Error::other(
                        "Owner disappeared before modeset dispatch",
                    )),
                );
                return;
            }
        };
        let registrations = self
            .resource_service
            .as_mut()
            .ok_or(ResourceError::InvalidState)
            .and_then(|service| match displaced_pool.as_ref() {
                Some((member, allocations)) => {
                    crate::kms::render::resources::register_kms_displacements(
                        commit,
                        *member,
                        allocations,
                        service,
                    )
                }
                None => Ok(Vec::new()),
            });
        let registrations = match registrations {
            Ok(registrations) => registrations,
            Err(error) => {
                let events = self
                    .platform
                    .owner_for(device)
                    .and_then(|owner| owner.cancel_live(commit).ok())
                    .unwrap_or_default();
                let _ = self.route_owner_event_batch(device, events, std::time::Instant::now());
                if let Some(conductor) = self.admission_conductors.get_mut(&device) {
                    let _ = conductor.admission.abort(token);
                }
                self.lifecycle_complete_client_modeset_without_dispatch(
                    device,
                    tag,
                    Err(std::io::Error::other(format!(
                        "Owner modeset dependencies were not ready: {error}"
                    ))),
                );
                return;
            }
        };
        if let Some(driver) = self.lifecycle_drivers.get_mut(&device) {
            driver
                .owner_commit_power_changes
                .insert(commit, owner_commit_power_changes);
            if !registrations.is_empty() {
                driver.kms_displacements.insert(commit, registrations);
            }
        }
        if !self.client_modeset_tag_current(device, tag)
            || !self.client_modeset_projection_current(device, pending.staged_projection.as_ref())
        {
            let events = self
                .platform
                .owner_for(device)
                .and_then(|owner| owner.cancel_live(commit).ok())
                .unwrap_or_default();
            let _ = self.route_owner_event_batch(device, events, std::time::Instant::now());
            if let Some(conductor) = self.admission_conductors.get_mut(&device) {
                let _ = conductor.admission.abort(token);
            }
            self.lifecycle_complete_queued_client_modeset(
                device,
                tag,
                self.lifecycle_superseding_kind(device),
            );
            return;
        }
        let marked_submitting = self
            .lifecycle_coordinator
            .client_modeset_submitting(&device, &tag)
            .unwrap_or(false);
        if !marked_submitting {
            let events = self
                .platform
                .owner_for(device)
                .and_then(|owner| owner.cancel_live(commit).ok())
                .unwrap_or_default();
            let _ = self.route_owner_event_batch(device, events, std::time::Instant::now());
            if let Some(conductor) = self.admission_conductors.get_mut(&device) {
                let _ = conductor.admission.abort(token);
            }
            self.lifecycle_complete_queued_client_modeset(
                device,
                tag,
                self.lifecycle_superseding_kind(device),
            );
            return;
        }
        if let Some(driver) = self.lifecycle_drivers.get_mut(&device) {
            if let Some(slot) = driver.client_modeset.as_mut()
                && slot.tag == tag
            {
                slot.phase = ClientModesetPhase::Dispatched;
                self.dispatched_client_modeset_tokens.insert(slot.token);
            }
            driver.client_modeset_commits.insert(commit, tag);
        }
        #[cfg(test)]
        if let Some(driver) = self.lifecycle_drivers.get_mut(&device) {
            driver.client_live_sends.push(tag);
        }
        let send = self
            .platform
            .devices
            .iter_mut()
            .find(|entry| entry.key == device)
            .and_then(|entry| {
                let owner = entry.owner.as_mut()?;
                let executor = entry.executor.as_mut()?;
                Some(owner.send_on(executor))
            });
        match send {
            Some(Ok(events)) => {
                let _ = self.admission_confirm(device, token, commit, pending.decision);
                let _ = self.route_owner_event_batch(device, events, std::time::Instant::now());
            }
            Some(Err(_)) | None => {
                if let Some(conductor) = self.admission_conductors.get_mut(&device) {
                    let _ = conductor.admission.abort(token);
                }
                let events = self
                    .platform
                    .owner_for(device)
                    .and_then(|owner| owner.cancel_live(commit).ok())
                    .unwrap_or_default();
                let _ = self.route_owner_event_batch(device, events, std::time::Instant::now());
            }
        }
    }

    fn lifecycle_finish_topology_validation(
        &mut self,
        device: DrmDeviceKey,
        validation_commit: CommitId,
        outcome: crate::kms::owner::device::ValidationOutcome,
    ) {
        if self.lifecycle_drivers.get(&device).is_some_and(|driver| {
            driver
                .pending_client_modeset_validations
                .contains_key(&validation_commit)
        }) {
            self.lifecycle_finish_client_modeset_validation(device, validation_commit, outcome);
            return;
        }
        let Some(mut pending) = self.lifecycle_drivers.get_mut(&device).and_then(|driver| {
            driver
                .pending_topology_validations
                .remove(&validation_commit)
        }) else {
            return;
        };
        let tag = pending.tag;
        if pending.cancelled || !self.lifecycle_tag_current(device, tag) {
            if let Some(owner) = self.platform.owner_for(device) {
                let _ = owner.abandon_validation(validation_commit);
            }
            if let Some(token) = pending.token.take()
                && let Some(conductor) = self.admission_conductors.get_mut(&device)
            {
                let _ = conductor.admission.abort(token);
            }
            self.lifecycle_dispose_topology_result(
                device,
                validation_commit,
                tag,
                TerminalState::FailedBeforeSubmit(FailureCause::IoctlRejected {
                    errno: libc::ECANCELED,
                }),
            );
            return;
        }
        match outcome {
            crate::kms::owner::device::ValidationOutcome::Passed => {
                self.lifecycle_submit_validated_topology(device, validation_commit, pending);
            }
            crate::kms::owner::device::ValidationOutcome::Rejected { errno } => {
                if let Some(owner) = self.platform.owner_for(device) {
                    let _ = owner.abandon_validation(validation_commit);
                }
                if let Some(token) = pending.token.take()
                    && let Some(conductor) = self.admission_conductors.get_mut(&device)
                {
                    let _ = conductor.admission.abort(token);
                }
                self.lifecycle_dispose_topology_result(
                    device,
                    validation_commit,
                    tag,
                    TerminalState::FailedBeforeSubmit(FailureCause::IoctlRejected { errno }),
                );
            }
            crate::kms::owner::device::ValidationOutcome::Abandoned(_) => {
                if let Some(owner) = self.platform.owner_for(device) {
                    let _ = owner.abandon_validation(validation_commit);
                }
                if let Some(token) = pending.token.take()
                    && let Some(conductor) = self.admission_conductors.get_mut(&device)
                {
                    let _ = conductor.admission.abort(token);
                }
                self.lifecycle_dispose_topology_result(
                    device,
                    validation_commit,
                    tag,
                    TerminalState::CompletionUnknown(
                        crate::kms::owner::record::UnknownCause::HostCall(
                            crate::kms::executor::UnknownReason::WatchdogExpired,
                        ),
                    ),
                );
            }
        }
    }

    fn lifecycle_submit_validated_topology(
        &mut self,
        device: DrmDeviceKey,
        validation_commit: CommitId,
        mut pending: PendingTopologyValidation,
    ) {
        let tag = pending.tag;
        let dpms_active = pending.dpms_active;
        let owner_commit_power_changes = pending
            .description
            .crtc_state
            .iter()
            .map(|state| OwnerCrtcPowerChange {
                incarnation: tag.incarnation,
                crtc: state.crtc_id,
                new_active: state.new_active,
                expected_completion: state.old_active || state.new_active,
            })
            .collect::<Vec<_>>();
        let decision = pending.decision.clone();
        let Some(token) = pending.token.take() else {
            return;
        };
        let expected_crtcs = pending
            .description
            .crtc_state
            .iter()
            .filter(|state| state.old_active || state.new_active)
            .map(|state| state.crtc_id)
            .collect::<Vec<_>>();
        let required_clock_crtcs = pending
            .description
            .crtc_state
            .iter()
            .filter(|state| state.old_active)
            .map(|state| state.crtc_id)
            .collect::<Vec<_>>();
        let clocks = match self.lifecycle_clock_readiness(device, &required_clock_crtcs) {
            LifecycleClockReadiness::Ready(clocks) => clocks,
            LifecycleClockReadiness::Waiting => {
                if let Some(owner) = self.platform.owner_for(device) {
                    let _ = owner.abandon_validation(validation_commit);
                }
                if let Some(conductor) = self.admission_conductors.get_mut(&device) {
                    let _ = conductor.admission.abort(token);
                }
                self.lifecycle_retry_topology_without_consuming_attempt(device, tag);
                self.promote_waiting_clock_probe(device);
                return;
            }
            LifecycleClockReadiness::Failed { crtc, key, outcome } => {
                if let Some(owner) = self.platform.owner_for(device) {
                    let _ = owner.abandon_validation(validation_commit);
                }
                if let Some(conductor) = self.admission_conductors.get_mut(&device) {
                    let _ = conductor.admission.abort(token);
                }
                if matches!(outcome, ProbeOutcome::Unknown(_)) {
                    return;
                }
                self.lifecycle_log_probe_failure_once(device, crtc, key, outcome);
                self.lifecycle_dispose_topology_result(
                    device,
                    validation_commit,
                    tag,
                    TerminalState::FailedBeforeSubmit(FailureCause::NeverDispatched(
                        RefusalCause::OwnerInternalError,
                    )),
                );
                return;
            }
            LifecycleClockReadiness::Missing(crtc) => {
                if let Some(owner) = self.platform.owner_for(device) {
                    let _ = owner.abandon_validation(validation_commit);
                }
                if let Some(conductor) = self.admission_conductors.get_mut(&device) {
                    let _ = conductor.admission.abort(token);
                }
                log::error!(
                    "lifecycle topology for {device:?} names served CRTC {crtc} without a clock record"
                );
                self.lifecycle_dispose_topology_result(
                    device,
                    validation_commit,
                    tag,
                    TerminalState::FailedBeforeSubmit(FailureCause::NeverDispatched(
                        RefusalCause::OwnerInternalError,
                    )),
                );
                return;
            }
            LifecycleClockReadiness::Inconsistent { crtc, key, detail } => {
                if let Some(owner) = self.platform.owner_for(device) {
                    let _ = owner.abandon_validation(validation_commit);
                }
                if let Some(conductor) = self.admission_conductors.get_mut(&device) {
                    let _ = conductor.admission.abort(token);
                }
                log::error!(
                    "lifecycle topology for {device:?} has inconsistent CRTC {crtc} clock {key:?}: {detail}"
                );
                self.lifecycle_dispose_topology_result(
                    device,
                    validation_commit,
                    tag,
                    TerminalState::FailedBeforeSubmit(FailureCause::NeverDispatched(
                        RefusalCause::OwnerInternalError,
                    )),
                );
                return;
            }
        };
        let mode_periods = expected_crtcs
            .iter()
            .map(|&crtc| (crtc, None))
            .collect::<BTreeMap<_, _>>();
        let completion_context = crate::kms::owner::completion::CompletionContext {
            class: crate::kms::owner::completion::CompletionClass::LifecycleInstallRestore,
            host_class: crate::kms::executor::HostCallClass::SeatActiveNonblock,
            allow_modeset: true,
            clocks,
            mode_periods,
            lifecycle_observed_max: None,
        };
        let begin_result = self.platform.owner_for(device).map(|owner| {
            owner.begin_validated_with_context(
                &pending.description,
                crate::kms::owner::ledger::Submitted::new(Vec::new(), Vec::new()),
                completion_context,
            )
        });
        let commit = match begin_result {
            Some(Ok((commit, _events))) => commit,
            Some(Err(error)) => {
                if let Some(owner) = self.platform.owner_for(device) {
                    let _ = owner.abandon_validation(validation_commit);
                }
                if let Some(conductor) = self.admission_conductors.get_mut(&device) {
                    let _ = conductor.admission.abort(token);
                }
                if Self::lifecycle_dispatch_error_is_transient(&error) {
                    self.lifecycle_retry_topology_without_consuming_attempt(device, tag);
                    self.promote_waiting_clock_probe(device);
                    return;
                }
                log::error!(
                    "validated lifecycle commit for {device:?} was refused before dispatch: {error}"
                );
                self.lifecycle_dispose_topology_result(
                    device,
                    validation_commit,
                    tag,
                    TerminalState::FailedBeforeSubmit(FailureCause::NeverDispatched(
                        RefusalCause::OwnerInternalError,
                    )),
                );
                return;
            }
            None => {
                if let Some(owner) = self.platform.owner_for(device) {
                    let _ = owner.abandon_validation(validation_commit);
                }
                if let Some(conductor) = self.admission_conductors.get_mut(&device) {
                    let _ = conductor.admission.abort(token);
                }
                log::error!(
                    "validated lifecycle commit for {device:?} was never dispatched: owner disappeared"
                );
                self.lifecycle_dispose_topology_result(
                    device,
                    validation_commit,
                    tag,
                    TerminalState::FailedBeforeSubmit(FailureCause::NeverDispatched(
                        RefusalCause::OwnerInternalError,
                    )),
                );
                return;
            }
        };
        let dependencies_ready = self.resource_service.as_mut().is_some_and(|service| {
            register_commit_dependencies(commit, Vec::new(), Vec::new(), service).is_ok()
        });
        if !dependencies_ready {
            let events = self
                .platform
                .owner_for(device)
                .and_then(|owner| owner.cancel_live(commit).ok())
                .unwrap_or_default();
            if let Some(conductor) = self.admission_conductors.get_mut(&device) {
                let _ = conductor.admission.abort(token);
            }
            self.lifecycle_drivers
                .get_mut(&device)
                .expect("Owner lifecycle driver")
                .topology_commits
                .insert(commit, tag);
            self.lifecycle_drivers
                .get_mut(&device)
                .expect("Owner lifecycle driver")
                .topology_dpms_active
                .insert(commit, dpms_active);
            self.lifecycle_drivers
                .get_mut(&device)
                .expect("Owner lifecycle driver")
                .owner_commit_power_changes
                .insert(commit, owner_commit_power_changes);
            let _ = self.route_owner_event_batch(device, events, std::time::Instant::now());
            return;
        }
        if let Some(driver) = self.lifecycle_drivers.get_mut(&device) {
            driver.topology_commits.insert(commit, tag);
            driver.topology_dpms_active.insert(commit, dpms_active);
            driver
                .owner_commit_power_changes
                .insert(commit, owner_commit_power_changes);
        }

        #[cfg(test)]
        self.lifecycle_apply_topology_hook(
            device,
            LifecycleTopologyTestHook::SupersedeBeforeLiveDispatch,
        );
        #[cfg(test)]
        self.lifecycle_apply_topology_hook(
            device,
            LifecycleTopologyTestHook::ReapExecutorBeforeLiveDispatch,
        );
        if !self.lifecycle_tag_current(device, tag) {
            #[cfg(test)]
            if let Some(driver) = self.lifecycle_drivers.get_mut(&device) {
                driver.stale_before_live_dispatch =
                    driver.stale_before_live_dispatch.saturating_add(1);
            }
            let events = self
                .platform
                .owner_for(device)
                .and_then(|owner| owner.cancel_live(commit).ok())
                .unwrap_or_default();
            if let Some(conductor) = self.admission_conductors.get_mut(&device) {
                let _ = conductor.admission.abort(token);
            }
            self.lifecycle_queue_input(
                device,
                ArbiterInput::CommitOutcome {
                    tag,
                    outcome: LifecycleCommitOutcome::Rejected {
                        topology_latched_generation: None,
                    },
                },
            );
            let _ = self.route_owner_event_batch(device, events, std::time::Instant::now());
            return;
        }

        #[cfg(test)]
        if let Some(driver) = self.lifecycle_drivers.get_mut(&device) {
            driver.live_sends.push(tag);
        }
        let send = self
            .platform
            .devices
            .iter_mut()
            .find(|entry| entry.key == device)
            .and_then(|entry| {
                let owner = entry.owner.as_mut()?;
                let executor = entry.executor.as_mut()?;
                Some(owner.send_on(executor))
            });
        match send {
            Some(Ok(events)) => {
                self.lifecycle_queue_input(
                    device,
                    ArbiterInput::CommitProgress {
                        tag,
                        progress: CommitProgress::Submitting,
                    },
                );
                let _ = self.admission_confirm(device, token, commit, decision);
                let _ = self.route_owner_event_batch(device, events, std::time::Instant::now());
            }
            Some(Err(error @ DispatchError::Refused { .. })) => {
                if !Self::lifecycle_dispatch_error_is_transient(&error) {
                    log::error!(
                        "validated lifecycle commit for {device:?} was refused before dispatch: {error}"
                    );
                }
                let _ = self.admission_dispose_refusal(
                    device,
                    token,
                    Admitted::Topology {
                        work: TopologyWork::Transition(tag),
                    },
                    error,
                );
            }
            Some(Err(error)) => {
                if let Some(conductor) = self.admission_conductors.get_mut(&device) {
                    let _ = conductor.admission.abort(token);
                }
                let events = self
                    .platform
                    .owner_for(device)
                    .and_then(|owner| owner.cancel_live(commit).ok())
                    .unwrap_or_default();
                log::error!(
                    "validated lifecycle commit for {device:?} failed before dispatch: {error}"
                );
                let _ = self.route_owner_event_batch(device, events, std::time::Instant::now());
            }
            None => {
                if let Some(conductor) = self.admission_conductors.get_mut(&device) {
                    let _ = conductor.admission.abort(token);
                }
                let events = self
                    .platform
                    .owner_for(device)
                    .and_then(|owner| owner.cancel_live(commit).ok())
                    .unwrap_or_default();
                log::error!(
                    "validated lifecycle commit for {device:?} was never dispatched: owner or executor disappeared"
                );
                let _ = self.route_owner_event_batch(device, events, std::time::Instant::now());
            }
        }
    }

    pub(super) fn lifecycle_promote_client_modeset(
        &mut self,
        device: DrmDeviceKey,
        mut slot: ClientModesetSlot,
    ) {
        use crate::kms::backend::ActiveOutput;
        #[cfg(test)]
        use crate::kms::render::admission::ClientModesetPromotionStep as Step;

        let key = crate::kms::backend::OutputKey::new(device, slot.connector.clone());
        let mut prepared = slot
            .prepared
            .take()
            .expect("a dispatched modeset retains its complete prepared set");
        let mut prepared_set = prepared.prepared_set;
        let mut identity_map = self.scene.stage_output_identity_map();
        let old_extent = (self.platform.fb_w, self.platform.fb_h);
        let mut lit_clock = None;
        let mut staged_projection_for_promotion = None;
        let mut retired_instance = None;

        if let Some(mode) = slot.mode {
            let output = prepared_set
                .output
                .take()
                .expect("configure promotion owns its discovered output");
            let output_instance = prepared_set
                .output_instance_id
                .take()
                .expect("configure promotion owns its reserved output instance");
            let scanout = prepared_set
                .scanout
                .take()
                .expect("configure promotion owns its registered scanout pool");
            let staged_scene = prepared_set
                .scene
                .take()
                .expect("configure promotion owns its staged scene state");
            let staged_projection = prepared
                .staged_projection
                .take()
                .expect("configure promotion owns its staged DPMS projection");
            let clock_key = prepared_set
                .clock_key
                .take()
                .expect("configure promotion owns its reserved CRTC clock epoch");
            lit_clock = (staged_projection.target == crate::kms::owner::lifecycle::DpmsTarget::On)
                .then_some(clock_key);
            staged_projection_for_promotion = Some(staged_projection);
            let scanout_route = scanout.route();
            let bo_count = scanout.display_pool().bos.len();
            assert!(
                bo_count != 0,
                "prepared scanout pool has a modeset front BO"
            );
            let hardware_crtc = clock_key.hardware_crtc;

            if let Some(output_idx) = self
                .platform
                .outputs
                .iter()
                .position(|output| output.key == key)
            {
                retired_instance = Some(self.platform.output_instance_ids[output_idx]);
                let retired_pool = self.platform.scanout_pools[output_idx]
                    .take()
                    .expect("a kept Owner output has its installed scanout pool");
                identity_map.replace(&key, staged_scene, retired_pool);
                let layout = &mut self.platform.outputs[output_idx];
                layout.output = output;
                layout.scanout_route = scanout_route;
                layout.x = slot.x;
                layout.y = slot.y;
                layout.width = mode.width;
                layout.height = mode.height;
                self.platform.scanout_pools[output_idx] = Some(scanout);
                self.platform.output_instance_ids[output_idx] = output_instance;
                self.platform.bo_generations[output_idx] = vec![Default::default(); bo_count];
                self.platform.first_pageflip_logged[output_idx] = false;
            } else {
                identity_map.insert(key.clone(), staged_scene);
                self.platform.outputs.push(ActiveOutput::new(
                    scanout_route,
                    output,
                    crate::drm::Swapchain::empty_for_tests(),
                    slot.x,
                    slot.y,
                ));
                self.platform.scanout_pools.push(Some(scanout));
                self.platform.output_instance_ids.push(output_instance);
                self.platform
                    .bo_generations
                    .push(vec![Default::default(); bo_count]);
                self.platform.first_pageflip_logged.push(false);
            }

            let entry = self.randr_id_alloc.entry_mut(&key);
            entry.config = crate::kms::render::backend::ConnectorConfig::Enabled {
                mode_w: mode.width,
                mode_h: mode.height,
                vrefresh: mode.vrefresh,
                x: slot.x,
                y: slot.y,
            };
            entry.crtc_associated = true;
            entry.client_configured = true;
            entry.connected = true;
            entry.last_enabled = None;

            let owner = self
                .platform
                .owner_for(device)
                .expect("current modeset owner remains installed through promotion");
            owner.install_modeset_clock_epoch(clock_key);
            self.next_present_crtc_clock_epoch = self.next_present_crtc_clock_epoch.max(
                clock_key
                    .epoch
                    .get()
                    .checked_add(1)
                    .expect("clock epoch reserved"),
            );
            assert_eq!(
                hardware_crtc,
                u32::from(
                    self.platform
                        .outputs
                        .iter()
                        .find(|output| output.key == key)
                        .expect("configured output was promoted")
                        .output
                        .crtc,
                )
            );
            let (fb_w, fb_h) = crate::kms::render::platform::recompute_fb_extent_from(
                &self
                    .platform
                    .outputs
                    .iter()
                    .map(|output| (output.x, output.y, output.width, output.height))
                    .collect::<Vec<_>>(),
            );
            self.platform.fb_w = fb_w;
            self.platform.fb_h = fb_h;
            self.update_input_extent(fb_w, fb_h);
            self.prune_armed_targets_to_live_outputs();
        } else {
            let output_idx = self
                .platform
                .outputs
                .iter()
                .position(|output| output.key == key)
                .expect("disable promotion retains its bound output until the boundary");
            retired_instance = Some(self.platform.output_instance_ids[output_idx]);
            self.commit_consumer.retire_current_for_crtc(
                crate::kms::render::platform::CrtcKey::for_output(
                    &self.platform.outputs[output_idx],
                ),
            );
            let retired_pool = self.platform.scanout_pools[output_idx]
                .take()
                .expect("a disabled Owner output has its installed scanout pool");
            identity_map.remove(&key, retired_pool);
            self.platform.outputs.remove(output_idx);
            self.platform.scanout_pools.remove(output_idx);
            self.platform.output_instance_ids.remove(output_idx);
            self.platform.bo_generations.remove(output_idx);
            self.platform.first_pageflip_logged.remove(output_idx);

            let entry = self.randr_id_alloc.entry_mut(&key);
            entry.config = crate::kms::render::backend::ConnectorConfig::Off;
            entry.crtc_associated = false;
            entry.client_configured = true;
            entry.last_enabled = None;

            let (fb_w, fb_h) = crate::kms::render::platform::recompute_fb_extent_from(
                &self
                    .platform
                    .outputs
                    .iter()
                    .map(|output| (output.x, output.y, output.width, output.height))
                    .collect::<Vec<_>>(),
            );
            self.platform.fb_w = fb_w;
            self.platform.fb_h = fb_h;
            self.update_input_extent(fb_w, fb_h);
            self.prune_armed_targets_to_live_outputs();
        }

        let root_extent_changed = old_extent != (self.platform.fb_w, self.platform.fb_h);
        let affected_devices = if root_extent_changed {
            self.lifecycle_conductors_devices_for_root_change()
        } else {
            std::collections::HashSet::from([device])
        };
        for affected in affected_devices {
            let _ = self.admission_advance_layout_generation(affected);
        }

        let has_live_device_outputs = self
            .platform
            .outputs
            .iter()
            .any(|output| output.key.device_key == device);
        let installed_active = if has_live_device_outputs {
            staged_projection_for_promotion
                .as_ref()
                .is_some_and(|projection| {
                    projection.target == crate::kms::owner::lifecycle::DpmsTarget::On
                })
                || (slot.mode.is_none() && self.owner_outputs_powered_on(device))
        } else {
            false
        };
        self.owner_dpms_installed_active
            .insert(device, installed_active);
        self.update_resource_service_activity();

        #[cfg(test)]
        if let Some(driver) = self.lifecycle_drivers.get_mut(&device) {
            driver.client_modeset_promotion_steps.push(Step::KmsState);
        }

        if let Some(projection) = staged_projection_for_promotion {
            #[cfg(test)]
            if let Some(driver) = self.lifecycle_drivers.get_mut(&device) {
                driver.client_modeset_promotion_steps.push(Step::Projection);
            }
            self.lifecycle_coordinator
                .commit_staged_protocol_output(&device, projection.output);
        } else {
            #[cfg(test)]
            if let Some(driver) = self.lifecycle_drivers.get_mut(&device) {
                driver.client_modeset_promotion_steps.push(Step::Projection);
            }
            self.lifecycle_coordinator
                .invalidate_staged_protocol_output(&device, &key);
        }

        self.scene
            .promote_output_identity_map(&self.platform, identity_map);
        if let Some(instance) = retired_instance {
            self.scene.retire_current_owner_buffer_in_bundle(instance);
        }
        #[cfg(test)]
        if let Some(driver) = self.lifecycle_drivers.get_mut(&device) {
            driver.client_modeset_promotion_steps.push(Step::Scene);
        }
        if root_extent_changed {
            self.scene.invalidate_all_scanout_damage();
        }
        self.scene.wake_for_damage();

        if let Some(key) = lit_clock {
            self.modeset_lit_clock_probes
                .insert((device, key.hardware_crtc, key.epoch.get()));
            self.retain_waiting_clock_probe(device, key);
            self.promote_waiting_clock_probe(device);
        }
        for affected in if root_extent_changed {
            self.lifecycle_conductors_devices_for_root_change()
        } else {
            std::collections::HashSet::from([device])
        } {
            let _ = self.admission_wake(affected, false);
        }
        prepared_set.mode_blob.take();
    }

    fn lifecycle_conductors_devices_for_root_change(
        &self,
    ) -> std::collections::HashSet<DrmDeviceKey> {
        self.admission_conductors.keys().copied().collect()
    }

    fn lifecycle_finish_client_modeset(
        &mut self,
        device: DrmDeviceKey,
        commit: CommitId,
        tag: ClientModesetTag<IncarnationId>,
        terminal: TerminalState,
    ) {
        let tracked_tag = self
            .lifecycle_drivers
            .get_mut(&device)
            .and_then(|driver| driver.client_modeset_commits.remove(&commit));
        if tracked_tag != Some(tag) {
            return;
        }
        if let Some(conductor) = self.admission_conductors.get_mut(&device) {
            conductor
                .admission
                .cancel_topology(TopologyWork::ClientModeset(tag));
        }
        let current = self.client_modeset_tag_current(device, tag);
        let Some(slot) = self.lifecycle_take_client_modeset_slot(device, tag) else {
            return;
        };
        let token = slot.token;
        let stale_success = terminal == TerminalState::Completed && !current;
        let terminal = if stale_success {
            TerminalState::CompletionUnknown(
                crate::kms::owner::record::UnknownCause::ContradictoryEvidence,
            )
        } else {
            terminal
        };
        let promoted = terminal == TerminalState::Completed;
        let (result, close_readiness, completion_unknown) = match terminal {
            TerminalState::Completed => {
                log::debug!(
                    "Owner RRSetCrtcConfig applied: output {} mode {:?} position ({}, {}) device {} modeset {}",
                    slot.connector,
                    slot.mode,
                    slot.x,
                    slot.y,
                    device,
                    slot.tag.modeset.get(),
                );
                self.lifecycle_promote_client_modeset(device, slot);
                (Ok(true), false, false)
            }
            TerminalState::FailedBeforeSubmit(FailureCause::IoctlRejected { errno }) => {
                let failure = if !current {
                    ClientModesetFailure::Stale
                } else {
                    ClientModesetFailure::KernelRejected { errno }
                };
                let close_readiness = if current {
                    if errno == libc::EBUSY
                        && let Some(driver) = self.lifecycle_drivers.get_mut(&device)
                    {
                        driver.client_modeset_foreign_busy = true;
                    }
                    if (errno == libc::EINVAL || errno == libc::EOPNOTSUPP)
                        && let Some(driver) = self.lifecycle_drivers.get_mut(&device)
                    {
                        driver.client_modeset_latch = Some(ClientModesetLatch {
                            topology_generation: tag.topology_generation,
                            output_id: slot.output_id,
                            connector: slot.connector.clone(),
                            mode: slot.mode,
                            x: slot.x,
                            y: slot.y,
                        });
                    }
                    !matches!(
                        errno,
                        libc::EINVAL | libc::EOPNOTSUPP | libc::ERANGE | libc::ENOSPC
                    )
                } else {
                    false
                };
                let result = client_modeset_error_for_slot(
                    device,
                    &slot,
                    Err(std::io::Error::other(failure)),
                );
                self.lifecycle_release_client_modeset_slot(slot);
                (result, close_readiness, false)
            }
            TerminalState::FailedBeforeSubmit(FailureCause::NeverDispatched(_cause)) => {
                let failure = if current {
                    ClientModesetFailure::OwnerRefused(OwnerRefusal::ReadinessClosed)
                } else {
                    ClientModesetFailure::Stale
                };
                let result = client_modeset_error_for_slot(
                    device,
                    &slot,
                    Err(std::io::Error::other(failure)),
                );
                self.lifecycle_release_client_modeset_slot(slot);
                (result, current, false)
            }
            TerminalState::CompletionUnknown(_cause) => {
                let failure = if current {
                    ClientModesetFailure::CompletionUnknown
                } else {
                    ClientModesetFailure::Stale
                };
                let result = client_modeset_error_for_slot(
                    device,
                    &slot,
                    Err(std::io::Error::other(failure)),
                );
                self.lifecycle_release_client_modeset_slot(slot);
                (result, false, true)
            }
        };
        if close_readiness {
            self.lifecycle_close_client_modeset_readiness(device, tag);
        }
        if completion_unknown {
            self.lifecycle_report_completion_loss(device);
        }
        self.lifecycle_end_client_modeset_direct_hold(device, tag, promoted);
        match self
            .lifecycle_coordinator
            .client_modeset_resolved(&device, &tag)
        {
            Ok(actions) => {
                let requester = self.lifecycle_current_tag(device);
                self.lifecycle_queue_actions(device, actions, requester);
            }
            Err(error) => log::error!(
                "Owner modeset result could not release lifecycle barrier for {device:?}: {error:?}"
            ),
        }
        self.complete_owner_client_modeset(token, result);
    }

    fn lifecycle_dispose_topology_result(
        &mut self,
        device: DrmDeviceKey,
        commit: CommitId,
        tag: TransitionTag<IncarnationId>,
        terminal: TerminalState,
    ) {
        let transient_refusal = matches!(
            terminal,
            TerminalState::FailedBeforeSubmit(FailureCause::NeverDispatched(
                RefusalCause::AlreadyInFlight
            ))
        );
        if !transient_refusal && let Some(conductor) = self.admission_conductors.get_mut(&device) {
            conductor
                .admission
                .cancel_topology(TopologyWork::Transition(tag));
        }
        let installed_dpms_active = self
            .lifecycle_drivers
            .get_mut(&device)
            .and_then(|driver| driver.topology_dpms_active.remove(&commit));
        if let Some(installed_active) = installed_dpms_active {
            let activate_clock_probes =
                installed_active && matches!(&terminal, TerminalState::Completed);
            match &terminal {
                TerminalState::Completed => {
                    self.owner_dpms_installed_active
                        .insert(device, installed_active);
                }
                TerminalState::CompletionUnknown(_) => {
                    // Keep serviced time moving until the device is rebuilt:
                    // an unknown physical outcome may have left scanout lit.
                    self.owner_dpms_installed_active.insert(device, true);
                }
                TerminalState::FailedBeforeSubmit(_) => {}
            }
            self.update_resource_service_activity();
            if activate_clock_probes {
                self.activate_admission_clock_probes(device);
            }
        }
        let current = self.lifecycle_tag_current(device, tag);
        #[cfg(test)]
        if !current && let Some(driver) = self.lifecycle_drivers.get_mut(&device) {
            driver.stale_results = driver.stale_results.saturating_add(1);
        }
        if current {
            match terminal {
                TerminalState::Completed => self.lifecycle_queue_input(
                    device,
                    ArbiterInput::CommitOutcome {
                        tag,
                        outcome: LifecycleCommitOutcome::Completed {
                            dpms_projections_retired: true,
                        },
                    },
                ),
                TerminalState::FailedBeforeSubmit(FailureCause::IoctlRejected { errno }) => {
                    let topology_latched_generation =
                        (errno == libc::EINVAL || errno == libc::EOPNOTSUPP).then(|| {
                            self.platform
                                .owner_ref(device)
                                .map(|owner| owner.topology_generation())
                                .unwrap_or(0)
                        });
                    self.lifecycle_queue_input(
                        device,
                        ArbiterInput::CommitOutcome {
                            tag,
                            outcome: LifecycleCommitOutcome::Rejected {
                                topology_latched_generation,
                            },
                        },
                    );
                }
                TerminalState::FailedBeforeSubmit(FailureCause::NeverDispatched(
                    RefusalCause::AlreadyInFlight,
                )) => {
                    self.lifecycle_retry_topology_without_consuming_attempt(device, tag);
                }
                TerminalState::FailedBeforeSubmit(FailureCause::NeverDispatched(_)) => {
                    self.lifecycle_queue_input(
                        device,
                        ArbiterInput::CommitOutcome {
                            tag,
                            outcome: LifecycleCommitOutcome::Rejected {
                                topology_latched_generation: None,
                            },
                        },
                    );
                }
                TerminalState::CompletionUnknown(_) => {
                    self.lifecycle_report_completion_loss(device);
                }
            }
        } else {
            match terminal {
                TerminalState::Completed => self.lifecycle_queue_input(
                    device,
                    ArbiterInput::CommitOutcome {
                        tag,
                        outcome: LifecycleCommitOutcome::Completed {
                            dpms_projections_retired: false,
                        },
                    },
                ),
                TerminalState::FailedBeforeSubmit(_) => self.lifecycle_queue_input(
                    device,
                    ArbiterInput::CommitOutcome {
                        tag,
                        outcome: LifecycleCommitOutcome::Rejected {
                            topology_latched_generation: None,
                        },
                    },
                ),
                TerminalState::CompletionUnknown(_) => {
                    self.lifecycle_report_completion_loss(device)
                }
            }
        }
        if let Some(driver) = self.lifecycle_drivers.get_mut(&device) {
            driver.topology_commits.remove(&commit);
            driver.topology_dpms_active.remove(&commit);
        }
    }

    fn lifecycle_report_completion_loss(&mut self, device: DrmDeviceKey) {
        if let Err(error) = self.lifecycle_register_owner_device(device) {
            log::error!("lifecycle completion loss registration for {device:?}: {error:?}");
            return;
        }
        match self.lifecycle_coordinator.report_completion_loss(&device) {
            Ok(dispatch) => {
                let requester = self.lifecycle_current_tag(device);
                self.lifecycle_queue_actions(device, dispatch.actions, requester);
            }
            Err(error) => log::error!("lifecycle completion loss for {device:?}: {error:?}"),
        }
    }

    pub(crate) fn lifecycle_begin_owner_event_batch(&mut self, device: DrmDeviceKey) {
        if self.lifecycle_owner_devices().contains(&device)
            && let Err(error) = self.lifecycle_register_owner_device(device)
        {
            log::error!("lifecycle owner-event registration for {device:?}: {error:?}");
            return;
        }
        if let Some(driver) = self.lifecycle_drivers.get_mut(&device) {
            driver.routing_batch_depth = driver.routing_batch_depth.saturating_add(1);
        }
    }

    pub(crate) fn lifecycle_end_owner_event_batch(&mut self, device: DrmDeviceKey) {
        if let Some(driver) = self.lifecycle_drivers.get_mut(&device) {
            driver.routing_batch_depth = driver.routing_batch_depth.saturating_sub(1);
        }
        self.lifecycle_drain_driver(device);
    }

    /// Validation replies are consumed by the lifecycle driver, while the
    /// other owner milestones still pass through the ordinary resource
    /// boundary before their lifecycle result is queued.
    pub(crate) fn lifecycle_before_owner_event(
        &mut self,
        device: DrmDeviceKey,
        event: &OwnerEvent<CommitResources>,
    ) -> bool {
        if let OwnerEvent::ValidationResolved { commit, outcome } = event
            && self.lifecycle_drivers.get(&device).is_some_and(|driver| {
                driver.pending_topology_validations.contains_key(commit)
                    || driver
                        .pending_client_modeset_validations
                        .contains_key(commit)
            })
        {
            if let Some(driver) = self.lifecycle_drivers.get_mut(&device) {
                driver.enqueue(LifecycleDriverWork::ValidationResolved {
                    commit: *commit,
                    outcome: *outcome,
                });
            }
            return true;
        }
        false
    }

    pub(crate) fn lifecycle_after_owner_event(
        &mut self,
        device: DrmDeviceKey,
        milestone: Option<LifecycleOwnerMilestone>,
        consumed: bool,
    ) {
        if !consumed {
            return;
        }
        match milestone {
            Some(LifecycleOwnerMilestone::Accepted(commit)) => {
                let tag = self
                    .lifecycle_drivers
                    .get(&device)
                    .and_then(|driver| driver.topology_commits.get(&commit).copied());
                if let Some(tag) = tag {
                    self.lifecycle_queue_input(
                        device,
                        ArbiterInput::CommitProgress {
                            tag,
                            progress: CommitProgress::Accepted,
                        },
                    );
                }
            }
            Some(LifecycleOwnerMilestone::Terminal(commit, terminal)) => {
                let transition_tag = self
                    .lifecycle_drivers
                    .get(&device)
                    .and_then(|driver| driver.topology_commits.get(&commit).copied());
                let client_tag = self
                    .lifecycle_drivers
                    .get(&device)
                    .and_then(|driver| driver.client_modeset_commits.get(&commit).copied());
                if let Some(tag) = transition_tag
                    && let Some(driver) = self.lifecycle_drivers.get_mut(&device)
                {
                    driver.enqueue(LifecycleDriverWork::TopologyTerminal {
                        commit,
                        tag,
                        terminal,
                    });
                } else if let Some(tag) = client_tag
                    && let Some(driver) = self.lifecycle_drivers.get_mut(&device)
                {
                    driver.enqueue(LifecycleDriverWork::ClientModesetTerminal {
                        commit,
                        tag,
                        terminal,
                    });
                }
            }
            None => {}
        }
    }

    pub(crate) fn lifecycle_owner_mechanism_failed(&mut self, device: DrmDeviceKey) {
        self.lifecycle_report_completion_loss(device);
    }

    pub(crate) fn lifecycle_owner_clock_probe_resolved(
        &mut self,
        device: DrmDeviceKey,
        outcome: ProbeOutcome,
    ) {
        if matches!(outcome, ProbeOutcome::Unknown(_)) {
            self.lifecycle_report_completion_loss(device);
        }
    }

    #[cfg(test)]
    pub(crate) fn lifecycle_inject_input_for_tests(
        &mut self,
        device: DrmDeviceKey,
        input: ArbiterInput<IncarnationId>,
    ) {
        self.lifecycle_queue_input(device, input);
    }

    pub(crate) fn lifecycle_resource_terminal_state(
        &self,
        device: DrmDeviceKey,
        commit: CommitId,
        terminal: TerminalState,
    ) -> TerminalState {
        let stale_success = terminal == TerminalState::Completed
            && self.lifecycle_drivers.get(&device).is_some_and(|driver| {
                driver
                    .topology_commits
                    .get(&commit)
                    .is_some_and(|tag| !self.lifecycle_tag_current(device, *tag))
                    || driver
                        .client_modeset_commits
                        .get(&commit)
                        .is_some_and(|tag| !self.client_modeset_tag_current(device, *tag))
            });
        if stale_success {
            TerminalState::CompletionUnknown(
                crate::kms::owner::record::UnknownCause::ContradictoryEvidence,
            )
        } else {
            terminal
        }
    }

    #[cfg(test)]
    fn lifecycle_apply_topology_hook(
        &mut self,
        device: DrmDeviceKey,
        expected: LifecycleTopologyTestHook,
    ) {
        let hook = self.lifecycle_drivers.get_mut(&device).and_then(|driver| {
            (driver.hook == Some(expected))
                .then(|| driver.hook.take())
                .flatten()
        });
        match hook {
            Some(LifecycleTopologyTestHook::SupersedeBeforeTestOnly)
            | Some(LifecycleTopologyTestHook::SupersedeBeforeLiveDispatch) => {
                let _ = self.lifecycle_set_dpms_power(
                    if self.lifecycle_coordinator.protocol_dpms_level() == 0 {
                        3
                    } else {
                        0
                    },
                );
            }
            Some(LifecycleTopologyTestHook::ReapExecutorBeforeLiveDispatch) => {
                if let Some(executor) = self
                    .platform
                    .devices
                    .iter_mut()
                    .find(|entry| entry.key == device)
                    .and_then(|entry| entry.executor.as_mut())
                {
                    executor.request_termination();
                    let _ = executor.try_reap();
                }
            }
            None => {}
        }
    }

    #[cfg(test)]
    pub(crate) fn admission_trace_for_tests(
        &self,
        device: DrmDeviceKey,
    ) -> Vec<AdmissionTraceStep> {
        self.admission_conductors
            .get(&device)
            .map_or_else(Vec::new, |conductor| conductor.trace.clone())
    }

    pub(crate) fn admission_is_active(&self, device: DrmDeviceKey) -> bool {
        self.admission_conductors.contains_key(&device)
            && self.platform.transport_gate(&device).is_some_and(|gate| {
                gate.state() == crate::kms::render::resources::TransportState::Owner
            })
    }

    /// Owner DPMS changes every CRTC in a device's projection together. Until
    /// the corresponding lifecycle commit completes, the last acknowledged
    /// physical state remains authoritative for ordinary admission.
    pub(crate) fn owner_outputs_powered_on(&self, device: DrmDeviceKey) -> bool {
        self.owner_dpms_installed_active
            .get(&device)
            .copied()
            .unwrap_or(true)
    }

    pub(crate) fn install_admission_conductor(
        &mut self,
        device: DrmDeviceKey,
        source: Box<dyn AdmissionSource>,
    ) {
        self.admission_conductors
            .insert(device, AdmissionConductor::new(source));
        self.activate_admission_clock_probes(device);
    }

    /// Install the fixture-only conductor. Production has no caller until
    /// stage 2c-iii converts the producers.
    #[cfg(test)]
    pub(crate) fn install_admission_conductor_for_tests(
        &mut self,
        device: DrmDeviceKey,
        source: Box<dyn AdmissionSource>,
    ) {
        self.admission_conductors.insert(
            device,
            AdmissionConductor::new_with_composed_backend(source, false),
        );
        self.activate_admission_clock_probes(device);
    }

    #[cfg(test)]
    pub(crate) fn install_admission_conductor_with_backend_composed_for_tests(
        &mut self,
        device: DrmDeviceKey,
        source: Box<dyn AdmissionSource>,
    ) {
        self.admission_conductors.insert(
            device,
            AdmissionConductor::new_with_composed_backend(source, true),
        );
        self.activate_admission_clock_probes(device);
    }

    pub(crate) fn admission_offer_composed(
        &mut self,
        device: DrmDeviceKey,
        crtc: CrtcId,
        generation: u64,
    ) -> Result<(), AdmissionError> {
        if !self.admission_is_active(device) {
            return Ok(());
        }
        let conductor = self
            .admission_conductors
            .get_mut(&device)
            .expect("active admission conductor");
        conductor.admission.set_composed(crtc, generation)?;
        conductor.composed.insert(crtc, generation);
        Ok(())
    }

    fn composed_output_index(&self, device: DrmDeviceKey, crtc: CrtcId) -> Option<usize> {
        let crtc = ::drm::control::from_u32::<::drm::control::crtc::Handle>(crtc)?;
        self.platform
            .outputs
            .iter()
            .position(|output| output.key.device_key == device && output.output.crtc == crtc)
    }

    fn composed_prepared_planes(
        &mut self,
        device: DrmDeviceKey,
        intents: &[(CrtcId, u64)],
    ) -> Option<Vec<crate::kms::render::composed_commit::ComposedPlane<'_>>> {
        if intents.is_empty() {
            return None;
        }
        let mut planes = Vec::with_capacity(intents.len());
        for &(crtc, generation) in intents {
            let output_idx = self.composed_output_index(device, crtc)?;
            let location = self.scene.owner_prepared_location(output_idx, generation)?;
            let bo_fb_handle = self
                .platform
                .scanout_pools
                .get(location.output_idx)
                .and_then(Option::as_ref)
                .and_then(|scanout| scanout.display_pool().bos.get(location.bo_idx))
                .and_then(|bo| bo.fb_handle);
            let framebuffer = {
                let service = self.resource_service.as_mut()?;
                self.scene
                    .owner_prepared_framebuffer(location, generation, bo_fb_handle, service)
                    .ok()??
            };
            planes.push(crate::kms::render::composed_commit::ComposedPlane {
                output: &self.platform.outputs[output_idx].output,
                framebuffer,
            });
        }
        Some(planes)
    }

    fn composed_property_ids(
        &mut self,
        device: DrmDeviceKey,
        intents: &[(CrtcId, u64)],
    ) -> Result<crate::kms::owner::closure::PropertyIds, String> {
        if intents.is_empty() {
            return Err("prepared composed generation is unavailable".to_string());
        }

        // Keep the mutable property cache borrow separate from the output and
        // scene borrows used to construct the builder's borrowed members.
        let devices = &mut self.platform.devices;
        let outputs = &self.platform.outputs;
        let scanout_pools = &self.platform.scanout_pools;
        let scene = &self.scene;
        let kms_device = devices
            .iter_mut()
            .find(|entry| entry.key == device)
            .ok_or_else(|| "composed device disappeared".to_string())?;

        let mut planes = Vec::with_capacity(intents.len());
        for &(crtc, generation) in intents {
            let output_idx = outputs
                .iter()
                .position(|output| {
                    output.key.device_key == device && u32::from(output.output.crtc) == crtc
                })
                .ok_or_else(|| "composed output disappeared".to_string())?;
            let location = scene
                .owner_prepared_location(output_idx, generation)
                .ok_or_else(|| "prepared composed generation is unavailable".to_string())?;
            let bo_fb_handle = scanout_pools
                .get(location.output_idx)
                .and_then(Option::as_ref)
                .and_then(|scanout| scanout.display_pool().bos.get(location.bo_idx))
                .and_then(|bo| bo.fb_handle);
            let framebuffer = self
                .resource_service
                .as_mut()
                .ok_or_else(|| "prepared composed resource service is unavailable".to_string())?;
            let framebuffer = scene
                .owner_prepared_framebuffer(location, generation, bo_fb_handle, framebuffer)
                .map_err(|error| format!("prepared composed framebuffer read failed: {error}"))?
                .ok_or_else(|| "prepared composed framebuffer is unavailable".to_string())?;
            planes.push(crate::kms::render::composed_commit::ComposedPlane {
                output: &outputs[output_idx].output,
                framebuffer,
            });
        }
        crate::kms::render::composed_commit::discover_composed_property_ids(
            &kms_device.device,
            &planes,
            &mut kms_device.active_property_cache,
        )
        .map_err(|error| error.to_string())
    }

    fn composed_readiness(
        &mut self,
        device: DrmDeviceKey,
        crtc: CrtcId,
        generation: u64,
    ) -> Readiness {
        let Some(output_idx) = self.composed_output_index(device, crtc) else {
            return Readiness::Waiting(WaitReason::SourceWaits);
        };
        if !self.scene.owner_composed_ready(output_idx, generation) {
            return Readiness::Waiting(WaitReason::SourceWaits);
        }
        if self
            .composed_property_ids(device, &[(crtc, generation)])
            .is_ok()
        {
            Readiness::Ready
        } else {
            // Property discovery is a readiness prerequisite. A missing or
            // inconsistent property never reaches owner begin/IPC.
            Readiness::Waiting(WaitReason::SourceWaits)
        }
    }

    fn composed_description(
        &mut self,
        device: DrmDeviceKey,
        decision: &AdmissionDecision,
    ) -> Result<crate::kms::owner::build::CommitDescription, String> {
        let intents = composed_intents(decision);
        let property_ids = self.composed_property_ids(device, &intents)?;
        let planes = self
            .composed_prepared_planes(device, &intents)
            .ok_or_else(|| "prepared composed generation is unavailable".to_string())?;
        Ok(crate::kms::render::composed_commit::composed_description(
            &planes,
            property_ids,
        ))
    }

    fn composed_resource_specs(
        &self,
        device: DrmDeviceKey,
        intents: &[(CrtcId, u64)],
    ) -> Option<Vec<ComposedResourceSpec>> {
        let topology_generation = self
            .platform
            .owner_ref(device)
            .map_or(0, |owner| owner.topology_generation());
        intents
            .iter()
            .map(|&(crtc, generation)| {
                let output_idx = self.composed_output_index(device, crtc)?;
                let location = self.scene.owner_prepared_location(output_idx, generation)?;
                let member = GroupMember::new(
                    CrtcKey::for_output(&self.platform.outputs[output_idx]),
                    topology_generation,
                    1,
                );
                Some(ComposedResourceSpec {
                    location,
                    generation,
                    member,
                })
            })
            .collect()
    }

    fn composed_homogeneous_group(&self, device: DrmDeviceKey) -> BTreeSet<CrtcId> {
        let outputs = self
            .platform
            .outputs
            .iter()
            .filter(|output| output.key.device_key == device)
            .collect::<Vec<_>>();
        let Some(first) = outputs.first() else {
            return BTreeSet::new();
        };
        let first_picked = first.output.picked.clone();
        outputs
            .into_iter()
            .filter(|output| effective_refresh_matches(&first_picked, &output.output.picked))
            .map(|output| u32::from(output.output.crtc))
            .collect()
    }

    /// Store the latest maintenance payload and queue its descriptor as one
    /// transaction. An unchanged generation is an omission: it clears the
    /// decider slot and leaves no desired payload in the conductor store.
    pub(crate) fn admission_offer_maintenance(
        &mut self,
        device: DrmDeviceKey,
        key: MaintenanceKey,
        payload: MaintenancePayload,
    ) -> Result<(), AdmissionError> {
        if !self.admission_is_active(device) {
            return Ok(());
        }

        let behind_commit = self
            .platform
            .owner_ref(device)
            .is_some_and(|owner| owner.slot().occupant().is_some());
        let Some(conductor) = self.admission_conductors.get_mut(&device) else {
            return Ok(());
        };
        conductor
            .admission
            .set_maintenance(key, payload.generation, behind_commit)?;

        if conductor.admission.maintenance(key).is_some() {
            conductor.maintenance.desired.insert(key, payload);
        } else {
            conductor.maintenance.desired.remove(&key);
        }
        Ok(())
    }

    /// Queue a direct candidate and its A1 descriptor as one transaction.
    /// The managed seam is not touched while an unflip barrier is pending.
    pub(crate) fn admission_offer_direct(
        &mut self,
        device: DrmDeviceKey,
        source_id: crate::kms::render::store::DrawableId,
        candidate: yserver_core::backend::PresentScanoutCandidate,
        event: yserver_core::backend::CompletedPresentEvent,
    ) -> Result<bool, crate::kms::render::resources::ResourceError> {
        if !self.admission_is_active(device) {
            return Ok(false);
        }
        if self
            .admission_conductors
            .get(&device)
            .expect("active admission conductor")
            .admission
            .unflip()
            .is_some()
        {
            return Ok(false);
        }

        let source_generation = {
            let conductor = self
                .admission_conductors
                .get_mut(&device)
                .expect("active admission conductor");
            let generation = conductor.next_direct_source_generation;
            conductor.next_direct_source_generation = generation
                .checked_add(1)
                .expect("direct source generation overflow");
            generation
        };
        let layout_generation = self
            .admission_conductors
            .get(&device)
            .expect("active admission conductor")
            .layout_generation;
        let topology_generation = self
            .platform
            .owner_ref(device)
            .map_or(0, |owner| owner.topology_generation());
        let crtcs = self
            .platform
            .outputs
            .iter()
            .filter(|output| output.key.device_key == device)
            .map(|output| u32::from(output.output.crtc))
            .collect::<BTreeSet<_>>();
        if crtcs.is_empty() {
            return Err(crate::kms::render::resources::ResourceError::InvalidState);
        }

        let prepared = self.managed_prepare_direct_candidate(source_id, candidate, event)?;
        if !prepared {
            return Ok(false);
        }
        self.managed_tag_queued_direct_successor(source_generation);

        let successor = DirectSuccessor {
            source_generation,
            layout_generation,
            topology_generation,
            crtcs,
        };
        let result = self
            .admission_conductors
            .get_mut(&device)
            .expect("active admission conductor")
            .admission
            .set_direct_successor(successor);
        match result {
            Ok(_displaced) => Ok(true),
            Err(_error) => {
                // The pre-check above makes this unreachable for a healthy
                // decider, but the two sides must still roll back together.
                self.managed_terminalize_queued_direct_successor(Some(source_generation));
                Err(crate::kms::render::resources::ResourceError::InvalidState)
            }
        }
    }

    /// Request the A1 unflip barrier and terminalize the exact queued direct
    /// frame it displaces. The exit-retirement/shadow work belongs to the
    /// unflip dispatch outside this task.
    pub(crate) fn admission_request_unflip(
        &mut self,
        device: DrmDeviceKey,
        crtcs: BTreeSet<CrtcId>,
    ) -> Result<(), AdmissionError> {
        if !self.admission_is_active(device) {
            return Ok(());
        }
        let displaced = self
            .admission_conductors
            .get_mut(&device)
            .expect("active admission conductor")
            .admission
            .request_unflip(crtcs)?;
        if let Some(successor) = displaced {
            let terminalized =
                self.managed_terminalize_queued_direct_successor(Some(successor.source_generation));
            debug_assert!(terminalized, "unflip descriptor must name its queued frame");
        }
        self.managed_publish_deferred_successor_skips_if_no_predecessor();
        Ok(())
    }

    /// Invalidate a queued direct successor whose layout generation is no
    /// longer current, then give another ready primary the admission slot.
    pub(crate) fn admission_note_layout_change(
        &mut self,
        device: DrmDeviceKey,
    ) -> AdmissionOutcome {
        let outcome = self.admission_advance_layout_generation(device);
        if outcome == AdmissionOutcome::TransportClosed {
            return outcome;
        }
        if outcome == AdmissionOutcome::Inert {
            return outcome;
        }
        self.admission_wake(device, false)
    }

    /// Advance one device's layout generation and invalidate its queued
    /// direct successor without waking admission. Topology promotion uses
    /// this form to finish all KMS, projection and scene moves before a
    /// newly eligible composed intent can dispatch.
    pub(crate) fn admission_advance_layout_generation(
        &mut self,
        device: DrmDeviceKey,
    ) -> AdmissionOutcome {
        if !self.admission_is_active(device) {
            return AdmissionOutcome::Inert;
        }

        let Some(layout_generation) = self
            .admission_conductors
            .get(&device)
            .and_then(|conductor| conductor.layout_generation.checked_add(1))
        else {
            if let Some(gate) = self.platform.transport_gate_mut(&device) {
                gate.force_close();
            }
            return AdmissionOutcome::TransportClosed;
        };

        let queued_source_generation = {
            let Some(conductor) = self.admission_conductors.get_mut(&device) else {
                return AdmissionOutcome::Inert;
            };
            conductor.layout_generation = layout_generation;
            conductor.admission.direct().and_then(|queued| {
                (queued.successor.layout_generation != layout_generation)
                    .then_some(queued.successor.source_generation)
            })
        };

        if let Some(source_generation) = queued_source_generation {
            let withdrawn = self
                .admission_conductors
                .get_mut(&device)
                .and_then(|conductor| conductor.admission.withdraw_direct(source_generation));
            if withdrawn.is_some() {
                let terminalized =
                    self.managed_terminalize_queued_direct_successor(Some(source_generation));
                if !terminalized {
                    if let Some(gate) = self.platform.transport_gate_mut(&device) {
                        gate.force_close();
                    }
                    return AdmissionOutcome::TransportClosed;
                }
            }
        }

        AdmissionOutcome::NothingAdmissible
    }

    /// Advance every live device's layout generation after a backend scene
    /// mutation. Layout changes are backend-global, while admission state is
    /// intentionally per DRM device; collect the keys first so a wake can
    /// mutate the conductors without holding a map borrow.
    pub(crate) fn admission_note_layout_change_all_devices(&mut self, reason: &'static str) {
        let devices: Vec<_> = self.admission_conductors.keys().copied().collect();
        for device in devices {
            self.admission_note_layout_change_for_device(device, reason);
        }
    }

    pub(crate) fn admission_note_layout_change_for_devices(
        &mut self,
        devices: &std::collections::HashSet<DrmDeviceKey>,
        reason: &'static str,
    ) {
        for device in devices.iter().copied() {
            self.admission_note_layout_change_for_device(device, reason);
        }
    }

    fn admission_note_layout_change_for_device(
        &mut self,
        device: DrmDeviceKey,
        reason: &'static str,
    ) {
        if matches!(
            self.admission_note_layout_change(device),
            AdmissionOutcome::TransportClosed
        ) {
            log::error!(
                "admission layout generation overflow or queued successor mismatch for {device:?}: {reason}"
            );
        }
    }

    /// Build the readiness input consumed by A1. This is mutable because an
    /// ineligible direct successor is invalidated at the first snapshot that
    /// observes the lost eligibility.
    pub(crate) fn admission_snapshot(
        &mut self,
        device: DrmDeviceKey,
        retirement_wake: bool,
    ) -> Option<ReadinessSnapshot> {
        if !self.admission_is_active(device) {
            return None;
        }
        let topology_generation = self
            .platform
            .owner_ref(device)
            .map_or(0, |owner| owner.topology_generation());
        let current_direct = has_current_direct(self);
        let ordinary_retirement_vacant = self
            .commit_consumer
            .capacity
            .is_vacant(crate::kms::render::resources::DirectRole::OrdinaryRetirement);
        let exit_retirement_vacant = self
            .commit_consumer
            .capacity
            .is_vacant(crate::kms::render::resources::DirectRole::ExitRetirement);
        let outputs_powered_on = self.owner_outputs_powered_on(device);
        let backend_composed = self
            .admission_conductors
            .get(&device)
            .is_some_and(|conductor| conductor.backend_composed);
        let composed_return_established = self.composed_return_established(device);
        let composed_intents = self
            .admission_conductors
            .get(&device)
            .map(|conductor| {
                conductor
                    .composed
                    .iter()
                    .map(|(&crtc, &generation)| (crtc, generation))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let backend_composed_readiness = if backend_composed {
            composed_intents
                .iter()
                .map(|&(crtc, generation)| {
                    (
                        (crtc, generation),
                        self.composed_readiness(device, crtc, generation),
                    )
                })
                .collect::<BTreeMap<_, _>>()
        } else {
            BTreeMap::new()
        };
        let backend_homogeneous_group = if backend_composed {
            self.composed_homogeneous_group(device)
        } else {
            BTreeSet::new()
        };
        let direct_source_generation = self
            .admission_conductors
            .get(&device)
            .and_then(|conductor| conductor.admission.direct())
            .map(|direct| direct.successor.source_generation);
        let direct_eligibility = direct_source_generation
            .map(|generation| self.direct_successor_eligibility(device, generation));
        let mut invalidate = None;
        let snapshot = {
            let conductor = self
                .admission_conductors
                .get_mut(&device)
                .expect("active admission conductor");
            let mut snapshot =
                ReadinessSnapshot::new(conductor.layout_generation, topology_generation);
            snapshot.retirement_wake = retirement_wake;

            let primary_intents = conductor
                .composed
                .iter()
                .map(|(&crtc, &generation)| IntentKey::Composed { crtc, generation })
                .chain(
                    conductor
                        .admission
                        .direct()
                        .map(|direct| IntentKey::Direct {
                            source_generation: direct.successor.source_generation,
                        }),
                )
                .collect::<Vec<_>>();

            for (&crtc, &generation) in &conductor.composed {
                snapshot.report(
                    IntentKey::Composed { crtc, generation },
                    if !outputs_powered_on {
                        Readiness::Waiting(WaitReason::OutputPoweredOff)
                    } else if backend_composed {
                        backend_composed_readiness
                            .get(&(crtc, generation))
                            .copied()
                            .unwrap_or(Readiness::Waiting(WaitReason::SourceWaits))
                    } else {
                        conductor
                            .source
                            .producer_readiness(IntentKey::Composed { crtc, generation })
                    },
                );
            }

            if let Some(direct) = conductor.admission.direct() {
                let generation = direct.successor.source_generation;
                let key = IntentKey::Direct {
                    source_generation: generation,
                };
                let eligibility = direct_eligibility
                    .unwrap_or_else(|| DirectEligibility::refused(conductor.layout_generation));
                let eligible = conductor.source.direct_eligible(generation, eligibility);
                let readiness = if !eligible {
                    invalidate = Some(generation);
                    Readiness::Waiting(WaitReason::NotDirectEligible)
                } else {
                    match conductor.source.producer_readiness(key) {
                        Readiness::Waiting(reason) => Readiness::Waiting(reason),
                        Readiness::Ready if current_direct && !ordinary_retirement_vacant => {
                            Readiness::Waiting(WaitReason::OrdinaryRetirementOccupied)
                        }
                        Readiness::Ready => Readiness::Ready,
                    }
                };
                snapshot.report(key, readiness);
            }

            for (&key, payload) in &conductor.maintenance.desired {
                let maintenance = IntentKey::Maintenance {
                    key,
                    generation: payload.generation,
                };
                snapshot.report(
                    maintenance,
                    conductor
                        .source
                        .maintenance_readiness(key, payload.generation),
                );
                for &primary in &primary_intents {
                    if conductor
                        .source
                        .compatible(key, payload.generation, primary)
                    {
                        snapshot.report_compatible(maintenance, primary);
                    }
                }
            }
            snapshot.homogeneous_group = if backend_composed {
                backend_homogeneous_group.clone()
            } else {
                conductor.source.homogeneous_group()
            };
            for &crtc in conductor.admission.cursor_recovery() {
                let readiness = if conductor.source.cursor_recovery_ready(crtc) {
                    Readiness::Ready
                } else {
                    Readiness::Waiting(WaitReason::SourceWaits)
                };
                snapshot.report(IntentKey::CursorRecovery { crtc }, readiness);
            }

            if conductor.admission.unflip().is_some() {
                let readiness = if !outputs_powered_on {
                    Readiness::Waiting(WaitReason::OutputPoweredOff)
                } else if !exit_retirement_vacant {
                    Readiness::Waiting(WaitReason::ExitRetirementOccupied)
                } else if !composed_return_established {
                    Readiness::Waiting(WaitReason::ComposedReturnNotEstablished)
                } else if !self.direct_unflip_shadow_ready() {
                    Readiness::Waiting(WaitReason::UnflipShadowNotMaterialized)
                } else {
                    Readiness::Ready
                };
                snapshot.report(IntentKey::Unflip, readiness);
            }
            snapshot
        };

        if let Some(source_generation) = invalidate {
            let withdrawn = self
                .admission_conductors
                .get_mut(&device)
                .expect("active admission conductor")
                .admission
                .withdraw_direct(source_generation);
            if withdrawn.is_some() {
                let terminalized =
                    self.managed_terminalize_queued_direct_successor(Some(source_generation));
                debug_assert!(
                    terminalized,
                    "ineligible descriptor must name its queued frame"
                );
            }
        }
        Some(snapshot)
    }

    /// Whether every output owned by this device has a composed framebuffer
    /// retained as the return target for a direct unflip.
    pub(crate) fn composed_return_established(&mut self, device: DrmDeviceKey) -> bool {
        #[cfg(test)]
        if let Some(override_result) = self.composed_return_test_override {
            return override_result;
        }

        let backend_composed = self
            .admission_conductors
            .get(&device)
            .is_some_and(|conductor| conductor.backend_composed);
        self.platform
            .outputs
            .iter()
            .enumerate()
            .filter(|(_, output)| output.key.device_key == device)
            .all(|(output_idx, _)| {
                if backend_composed {
                    let bo_fb_handle = self
                        .platform
                        .scanout_pools
                        .get(output_idx)
                        .and_then(Option::as_ref)
                        .and_then(|scanout| scanout.display_pool().bos.first())
                        .and_then(|bo| bo.fb_handle);
                    self.resource_service
                        .as_mut()
                        .and_then(|service| {
                            self.scene
                                .owner_current_framebuffer(output_idx, bo_fb_handle, service)
                                .ok()
                        })
                        .flatten()
                        .is_some()
                } else {
                    self.platform
                        .retained_composed_framebuffer(output_idx)
                        .is_some()
                }
            })
    }

    pub(crate) fn direct_entry_composed_return_established(
        &mut self,
        device: DrmDeviceKey,
    ) -> Option<bool> {
        (self.admission_is_active(device) && !has_current_direct(self))
            .then(|| self.composed_return_established(device))
    }

    /// Perform one complete admission transaction. The decider remains pure
    /// until the owner send boundary: `lock` only creates a token, while
    /// `confirm` is reached solely after `send_on` reports success.
    pub(crate) fn admission_wake(
        &mut self,
        device: DrmDeviceKey,
        retirement_wake: bool,
    ) -> AdmissionOutcome {
        self.promote_waiting_clock_probe(device);
        let recovery_stopped = self
            .admission_conductors
            .get(&device)
            .is_some_and(|conductor| conductor.recovery_stopped);
        let lifecycle_blocks_ordinary =
            self.admission_conductors
                .get(&device)
                .is_some_and(|conductor| {
                    conductor.lifecycle_admission_closed && conductor.admission.topology().is_none()
                });
        let outcome = if !self.admission_is_active(device) || recovery_stopped {
            AdmissionOutcome::Inert
        } else if lifecycle_blocks_ordinary {
            AdmissionOutcome::NothingAdmissible
        } else if self
            .platform
            .owner_ref(device)
            .is_none_or(|owner| !owner.slot().is_idle())
            || self
                .admission_conductors
                .get(&device)
                .is_some_and(|conductor| conductor.admission.is_locked())
        {
            AdmissionOutcome::SlotBusy
        } else {
            match self.admission_snapshot(device, retirement_wake) {
                None => AdmissionOutcome::Inert,
                Some(snapshot) => match self
                    .admission_conductors
                    .get(&device)
                    .expect("active admission conductor")
                    .admission
                    .decide(&snapshot)
                {
                    None => AdmissionOutcome::NothingAdmissible,
                    Some(decision) => {
                        #[cfg(test)]
                        if let Some(conductor) = self.admission_conductors.get_mut(&device) {
                            conductor.trace.push(AdmissionTraceStep::Decided);
                        }
                        #[cfg(test)]
                        self.admission_force_lock_mismatch_if_requested(device, &decision);

                        match self
                            .admission_conductors
                            .get_mut(&device)
                            .expect("active admission conductor")
                            .admission
                            .lock(decision.clone(), &snapshot)
                        {
                            Ok(token) => self.admission_dispatch_decision(device, token, decision),
                            Err(_error) => {
                                self.platform
                                    .transport_gate_mut(&device)
                                    .expect("active admission transport gate")
                                    .force_close();
                                AdmissionOutcome::TransportClosed
                            }
                        }
                    }
                },
            }
        };

        if self.admission_is_active(device) {
            self.managed_publish_deferred_successor_skips_if_no_predecessor();
        }
        outcome
    }

    fn admission_dispatch_decision(
        &mut self,
        device: DrmDeviceKey,
        token: AdmissionToken,
        decision: AdmissionDecision,
    ) -> AdmissionOutcome {
        if matches!(decision.admitted, Admitted::Topology { .. }) {
            self.admission_dispatch_topology(device, token, decision)
        } else if decision_requires_unsupported(&decision) {
            self.admission_abort_unsupported(device, token, &decision)
        } else if matches!(decision.admitted, Admitted::Unflip { .. }) {
            let outcome = self.admission_dispatch_unflip(device, token, decision);
            if matches!(
                outcome,
                AdmissionOutcome::PreparationRefused
                    | AdmissionOutcome::BeginRefused
                    | AdmissionOutcome::TransportClosed
                    | AdmissionOutcome::Unsupported(_)
            ) {
                self.lifecycle_client_modeset_unflip_dispatch_failed(device, None);
            }
            outcome
        } else if matches!(decision_primary(&decision), Some(Admitted::Direct { .. })) {
            self.admission_dispatch_direct(device, token, decision)
        } else if matches!(
            decision_primary(&decision),
            Some(Admitted::Composed { .. } | Admitted::Bundle { .. }) | None
        ) {
            self.admission_dispatch_primary(device, token, decision)
        } else {
            self.admission_abort(device, token);
            AdmissionOutcome::Unsupported(decision.tier)
        }
    }

    fn admission_dispatch_unflip(
        &mut self,
        device: DrmDeviceKey,
        token: AdmissionToken,
        decision: AdmissionDecision,
    ) -> AdmissionOutcome {
        let members = match &decision.admitted {
            Admitted::Unflip { crtcs } => {
                match crate::kms::render::unflip_owner::members(self, device, crtcs) {
                    Ok(members) => members,
                    Err(_error) => {
                        self.admission_abort(device, token);
                        return AdmissionOutcome::PreparationRefused;
                    }
                }
            }
            _ => {
                self.admission_abort(device, token);
                return AdmissionOutcome::Unsupported(decision.tier);
            }
        };
        let desc =
            match crate::kms::render::unflip_owner::description(self, device, &decision.admitted) {
                Ok(desc) => desc,
                Err(_error) => {
                    self.admission_abort(device, token);
                    return AdmissionOutcome::PreparationRefused;
                }
            };
        let new_resources =
            match crate::kms::render::unflip_owner::resources(self, device, &decision.admitted) {
                Ok(resources) => resources,
                Err(_error) => {
                    self.admission_abort(device, token);
                    return AdmissionOutcome::PreparationRefused;
                }
            };
        if self.resource_service.is_none() {
            self.admission_abort(device, token);
            return AdmissionOutcome::BeginRefused;
        }

        let exit_slot = match self
            .commit_consumer
            .capacity
            .reserve(crate::kms::render::resources::DirectRole::ExitRetirement)
        {
            Ok(slot) => slot,
            Err(_error) => {
                self.admission_abort(device, token);
                return AdmissionOutcome::PreparationRefused;
            }
        };
        let mut old_resources = match self.commit_consumer.take_current_for_members(&members) {
            Ok(old) if !old.is_empty() => old,
            Ok(old) => {
                self.discharge_bare_reservation(exit_slot);
                self.commit_consumer.current_resources.extend(old);
                self.admission_abort(device, token);
                return AdmissionOutcome::PreparationRefused;
            }
            Err(_error) => {
                self.discharge_bare_reservation(exit_slot);
                self.admission_abort(device, token);
                return AdmissionOutcome::PreparationRefused;
            }
        };
        let Some(current) = old_resources.iter_mut().find_map(|resources| {
            resources
                .direct_role
                .as_mut()
                .filter(|role| role.role() == crate::kms::render::resources::DirectRole::Current)
        }) else {
            self.discharge_bare_reservation(exit_slot);
            self.commit_consumer.current_resources.extend(old_resources);
            self.admission_abort(device, token);
            return AdmissionOutcome::PreparationRefused;
        };
        if let Err((_error, leaked)) = self
            .commit_consumer
            .capacity
            .move_into_reserved(current, exit_slot)
        {
            self.discharge_bare_reservation(leaked);
            self.commit_consumer.current_resources.extend(old_resources);
            self.admission_abort(device, token);
            return AdmissionOutcome::PreparationRefused;
        }

        let mut old_state = Some(old_resources);
        let mut new_state = Some(new_resources);
        let result = {
            let consumer = &mut self.commit_consumer;
            let service = self
                .resource_service
                .as_mut()
                .expect("resource service checked above");
            let Some(device_entry) = self
                .platform
                .devices
                .iter_mut()
                .find(|entry| entry.key == device)
            else {
                let old = old_state.take().expect("unflip old state");
                consumer.current_resources.extend(old);
                return self.admission_handle_dispatch_failure(
                    device,
                    token,
                    DispatchFailureRoute::Unflip { members },
                    DispatchFailureResources::Refused {
                        new: new_state.take().unwrap_or_default(),
                    },
                );
            };
            let Some(owner) = device_entry.owner.as_mut() else {
                let old = old_state.take().expect("unflip old state");
                consumer.current_resources.extend(old);
                return self.admission_handle_dispatch_failure(
                    device,
                    token,
                    DispatchFailureRoute::Unflip { members },
                    DispatchFailureResources::Refused {
                        new: new_state.take().unwrap_or_default(),
                    },
                );
            };
            owner.begin_with_fallible_ledger(&desc, |commit| {
                let old = old_state.take().ok_or((
                    crate::kms::render::resources::ResourceError::InvalidState,
                    Vec::new(),
                    Vec::new(),
                ))?;
                let Some(new) = new_state.take() else {
                    return Err((
                        crate::kms::render::resources::ResourceError::InvalidState,
                        old,
                        Vec::new(),
                    ));
                };
                let new = new
                    .into_iter()
                    .map(|resources| {
                        resources.with_commit_id(crate::kms::render::resources::CommitKey::new(
                            device, commit,
                        ))
                    })
                    .collect::<Vec<_>>();
                register_commit_dependencies(commit, old, new, service)
            })
        };

        let commit = match result {
            Ok((commit, _events)) => commit,
            Err(FallibleBeginError::Ledger((_error, old, new))) => {
                return self.admission_handle_dispatch_failure(
                    device,
                    token,
                    DispatchFailureRoute::Unflip { members },
                    DispatchFailureResources::Ledger { old, new },
                );
            }
            Err(FallibleBeginError::Cleanup {
                error: (_error, old, new),
                ..
            }) => {
                return self.admission_handle_dispatch_failure(
                    device,
                    token,
                    DispatchFailureRoute::Unflip { members },
                    DispatchFailureResources::Cleanup { old, new },
                );
            }
            Err(FallibleBeginError::Refused { .. }) => {
                let old = old_state.take().expect("refused unflip old state");
                self.commit_consumer.current_resources.extend(old);
                let restored = self.restore_unflip_dispatch_resources(
                    new_state.take().unwrap_or_default(),
                    &members,
                );
                self.admission_abort(device, token);
                if restored == ResourceRestore::Succeeded {
                    return AdmissionOutcome::BeginRefused;
                }
                if let Some(gate) = self.platform.transport_gate_mut(&device) {
                    gate.force_close();
                }
                return AdmissionOutcome::TransportClosed;
            }
        };
        let send_result = {
            let device_entry = self
                .platform
                .devices
                .iter_mut()
                .find(|entry| entry.key == device)
                .expect("admission device");
            let owner = device_entry.owner.as_mut().expect("admission owner");
            owner.send_on(device_entry.executor.as_mut().expect("admission executor"))
        };
        let affected_outputs = members
            .iter()
            .filter_map(|member| {
                self.platform
                    .outputs
                    .iter()
                    .find(|output| {
                        crate::kms::render::platform::CrtcKey::for_output(output) == member.crtc
                    })
                    .map(|output| output.key.clone())
            })
            .collect::<Vec<_>>();
        match send_result {
            Ok(_events) => {
                let outcome = self.admission_confirm(device, token, commit, decision);
                if matches!(outcome, AdmissionOutcome::Dispatched(_)) {
                    self.record_owner_unflip_return(
                        crate::kms::render::resources::CommitKey::new(device, commit),
                        affected_outputs,
                    );
                }
                outcome
            }
            Err(error @ DispatchError::Refused { .. }) => {
                self.admission_dispose_refusal(device, token, decision.admitted, error)
            }
            Err(_error) => {
                self.admission_abort(device, token);
                AdmissionOutcome::BeginRefused
            }
        }
    }

    fn admission_abort_unsupported(
        &mut self,
        device: DrmDeviceKey,
        token: AdmissionToken,
        decision: &AdmissionDecision,
    ) -> AdmissionOutcome {
        self.admission_abort(device, token);
        AdmissionOutcome::Unsupported(decision.tier)
    }

    #[cfg(test)]
    pub(crate) fn admission_dispatch_decision_for_tests(
        &mut self,
        device: DrmDeviceKey,
        token: AdmissionToken,
        decision: &AdmissionDecision,
    ) -> AdmissionOutcome {
        self.admission_dispatch_decision(device, token, decision.clone())
    }

    fn restore_direct_dispatch_resources(
        &mut self,
        mut new: Vec<CommitResources>,
        retirement: Option<crate::kms::render::resources::RoleReservation>,
    ) -> ResourceRestore {
        let Some(resources) = new.pop() else {
            return ResourceRestore::Missing;
        };
        if !new.is_empty() {
            return ResourceRestore::Extra;
        }
        if self
            .managed_undo_direct_dispatch(PreparedDirectDispatch {
                resources,
                retirement,
            })
            .is_ok()
        {
            ResourceRestore::Succeeded
        } else {
            ResourceRestore::Failed
        }
    }

    fn restore_unflip_dispatch_resources(
        &mut self,
        new: Vec<CommitResources>,
        members: &[GroupMember],
    ) -> ResourceRestore {
        if new.iter().any(|resources| resources.direct_role.is_some()) {
            return ResourceRestore::Extra;
        }
        let Some(resources) = self
            .commit_consumer
            .current_resources
            .iter_mut()
            .find(|resources| {
                members
                    .iter()
                    .all(|member| resources.crtcs.contains(member))
            })
        else {
            return ResourceRestore::Missing;
        };
        let Some(role) = resources.direct_role.as_mut() else {
            return ResourceRestore::Missing;
        };
        if role.role() != crate::kms::render::resources::DirectRole::ExitRetirement {
            return ResourceRestore::Failed;
        }
        if self
            .commit_consumer
            .capacity
            .move_role(role, crate::kms::render::resources::DirectRole::Current)
            .is_err()
        {
            ResourceRestore::Failed
        } else {
            ResourceRestore::Succeeded
        }
    }

    fn admission_handle_dispatch_failure(
        &mut self,
        device: DrmDeviceKey,
        token: AdmissionToken,
        route: DispatchFailureRoute,
        failure: DispatchFailureResources,
    ) -> AdmissionOutcome {
        let (kind, old, new) = failure.split();
        self.commit_consumer.current_resources.extend(old);

        let (route_kind, restore) = match route {
            DispatchFailureRoute::Primary { backend_composed } => {
                let restore = if matches!(kind, DispatchFailureKind::Refused) {
                    ResourceRestore::NotAttempted
                } else if self.restore_primary_composed_resources(device, backend_composed, new) {
                    ResourceRestore::Succeeded
                } else {
                    ResourceRestore::Failed
                };
                (DispatchFailureRouteKind::Primary, restore)
            }
            DispatchFailureRoute::Direct { retirement } => (
                DispatchFailureRouteKind::Direct,
                self.restore_direct_dispatch_resources(new, retirement),
            ),
            DispatchFailureRoute::Unflip { members } => (
                DispatchFailureRouteKind::Unflip,
                self.restore_unflip_dispatch_resources(new, &members),
            ),
        };
        let action = dispatch_failure_policy(route_kind, kind, restore);
        if action.outcome == DispatchFailureOutcome::TransportClosed {
            log::error!(
                "admission dispatch failure is fail-closed: route={route_kind:?}, kind={kind:?}, resource_restore={restore:?}"
            );
        }
        if action.close_gate
            && let Some(gate) = self.platform.transport_gate_mut(&device)
        {
            gate.force_close();
        }
        self.admission_abort(device, token);
        match action.outcome {
            DispatchFailureOutcome::BeginRefused => AdmissionOutcome::BeginRefused,
            DispatchFailureOutcome::TransportClosed => AdmissionOutcome::TransportClosed,
        }
    }

    fn admission_dispatch_primary(
        &mut self,
        device: DrmDeviceKey,
        token: AdmissionToken,
        decision: crate::kms::owner::admission::AdmissionDecision,
    ) -> AdmissionOutcome {
        let primary = decision_primary(&decision).cloned();
        if let Some(Admitted::Bundle { members }) = &primary
            && !members
                .iter()
                .all(|member| matches!(member, Admitted::Composed { .. }))
        {
            self.admission_abort(device, token);
            return AdmissionOutcome::Unsupported(decision.tier);
        }
        let backend_composed = self
            .admission_conductors
            .get(&device)
            .is_some_and(|conductor| conductor.backend_composed);
        let intents = composed_intents(&decision);
        let use_backend_composed = backend_composed && !intents.is_empty();
        let resource_specs = if use_backend_composed {
            let Some(specs) = self.composed_resource_specs(device, &intents) else {
                self.admission_abort(device, token);
                return AdmissionOutcome::PreparationRefused;
            };
            specs
        } else {
            Vec::new()
        };
        let desc = if use_backend_composed {
            match self.composed_description(device, &decision) {
                Ok(desc) => desc,
                Err(_error) => {
                    // Property discovery and framebuffer lookup are
                    // readiness prerequisites. A generation that becomes
                    // unavailable between snapshot and dispatch is refused
                    // without consuming its prepared lease.
                    self.admission_abort(device, token);
                    return AdmissionOutcome::PreparationRefused;
                }
            }
        } else {
            let Some(conductor) = self.admission_conductors.get_mut(&device) else {
                return AdmissionOutcome::TransportClosed;
            };
            conductor.source.describe(&decision)
        };
        let result: Result<CommitId, DispatchFailureResources> = {
            let consumer = &mut self.commit_consumer;
            let Some(service) = self.resource_service.as_mut() else {
                self.admission_abort(device, token);
                return AdmissionOutcome::BeginRefused;
            };
            let Some(device_entry) = self
                .platform
                .devices
                .iter_mut()
                .find(|entry| entry.key == device)
            else {
                self.admission_abort(device, token);
                return AdmissionOutcome::BeginRefused;
            };
            let Some(owner) = device_entry.owner.as_mut() else {
                self.admission_abort(device, token);
                return AdmissionOutcome::BeginRefused;
            };
            let scene = &mut self.scene;
            let scanout_pools = &mut self.platform.scanout_pools;
            if use_backend_composed {
                let specs = resource_specs;
                match owner.begin_with_fallible_ledger(&desc, move |commit| {
                    let mut new = Vec::with_capacity(specs.len());
                    for spec in specs.iter().copied() {
                        let mut resources = match scene.take_owner_composed_resources(
                            spec.location,
                            spec.generation,
                            spec.member,
                            commit,
                            scanout_pools,
                        ) {
                            Ok(resources) => resources,
                            Err(error) => return Err((error, Vec::new(), new)),
                        };
                        new.append(&mut resources);
                    }
                    let new = new
                        .into_iter()
                        .map(|resources| {
                            resources.with_commit_id(crate::kms::render::resources::CommitKey::new(
                                device, commit,
                            ))
                        })
                        .collect::<Vec<_>>();
                    let members = new
                        .iter()
                        .flat_map(|resources| resources.crtcs.iter().copied())
                        .collect::<Vec<_>>();
                    let old = match consumer.take_current_for_members(&members) {
                        Ok(old) => old,
                        Err(error) => return Err((error, Vec::new(), new)),
                    };
                    let submitted = match register_commit_dependencies(commit, old, new, service) {
                        Ok(submitted) => submitted,
                        Err(error) => return Err(error),
                    };
                    let transaction_specs = specs
                        .iter()
                        .map(|spec| (spec.location, spec.generation, spec.member))
                        .collect::<Vec<_>>();
                    if !scene.install_owner_damage_transaction(
                        crate::kms::render::resources::CommitKey::new(device, commit),
                        &transaction_specs,
                    ) {
                        let (old, new) = submitted.into_parts();
                        return Err((ResourceError::InvalidState, old, new));
                    }
                    Ok(submitted)
                }) {
                    Ok((commit, _events)) => Ok(commit),
                    Err(FallibleBeginError::Ledger((_error, old, new))) => {
                        Err(DispatchFailureResources::Ledger { old, new })
                    }
                    Err(FallibleBeginError::Cleanup {
                        error: (_error, old, new),
                        ..
                    }) => Err(DispatchFailureResources::Cleanup { old, new }),
                    Err(FallibleBeginError::Refused { .. }) => {
                        Err(DispatchFailureResources::Refused { new: Vec::new() })
                    }
                }
            } else {
                let Some(conductor) = self.admission_conductors.get_mut(&device) else {
                    self.admission_abort(device, token);
                    return AdmissionOutcome::TransportClosed;
                };
                let source = &mut conductor.source;
                match owner.begin_with_fallible_ledger(&desc, |commit| {
                    let new = match primary.as_ref() {
                        Some(Admitted::Composed { crtc, generation }) => {
                            source.composed_resources(*crtc, *generation)
                        }
                        Some(Admitted::Bundle { members }) => members
                            .iter()
                            .filter_map(|member| match member {
                                Admitted::Composed { crtc, generation } => {
                                    Some(source.composed_resources(*crtc, *generation))
                                }
                                _ => None,
                            })
                            .flatten()
                            .collect(),
                        None => Vec::new(),
                        Some(_) => Vec::new(),
                    };
                    let new = new
                        .into_iter()
                        .map(|resources| {
                            resources.with_commit_id(crate::kms::render::resources::CommitKey::new(
                                device, commit,
                            ))
                        })
                        .collect::<Vec<_>>();
                    let members = new
                        .iter()
                        .flat_map(|resources| resources.crtcs.iter().copied())
                        .collect::<Vec<_>>();
                    let old = match consumer.take_current_for_members(&members) {
                        Ok(old) => old,
                        Err(error) => return Err((error, Vec::new(), new)),
                    };
                    register_commit_dependencies(commit, old, new, service)
                }) {
                    Ok((commit, _events)) => Ok(commit),
                    Err(FallibleBeginError::Ledger((_error, old, new))) => {
                        Err(DispatchFailureResources::Ledger { old, new })
                    }
                    Err(FallibleBeginError::Cleanup {
                        error: (_error, old, new),
                        ..
                    }) => Err(DispatchFailureResources::Cleanup { old, new }),
                    Err(FallibleBeginError::Refused { .. }) => {
                        Err(DispatchFailureResources::Refused { new: Vec::new() })
                    }
                }
            }
        };

        let commit = match result {
            Ok(commit) => commit,
            Err(failure) => {
                return self.admission_handle_dispatch_failure(
                    device,
                    token,
                    DispatchFailureRoute::Primary {
                        backend_composed: use_backend_composed,
                    },
                    failure,
                );
            }
        };
        #[cfg(not(test))]
        let _ = commit;
        let send_result = {
            let Some(device_entry) = self
                .platform
                .devices
                .iter_mut()
                .find(|entry| entry.key == device)
            else {
                self.admission_abort(device, token);
                return AdmissionOutcome::BeginRefused;
            };
            let Some(owner) = device_entry.owner.as_mut() else {
                self.admission_abort(device, token);
                return AdmissionOutcome::BeginRefused;
            };
            let Some(executor) = device_entry.executor.as_mut() else {
                self.admission_abort(device, token);
                return AdmissionOutcome::BeginRefused;
            };
            owner.send_on(executor)
        };
        match send_result {
            Ok(_events) => {
                let outcome = self.admission_confirm(device, token, commit, decision);
                #[cfg(test)]
                if matches!(outcome, AdmissionOutcome::Dispatched(_))
                    && let Some(conductor) = self.admission_conductors.get_mut(&device)
                {
                    conductor.trace.push(AdmissionTraceStep::Dispatched(commit));
                }
                outcome
            }
            Err(error @ DispatchError::Refused { .. }) => {
                let admitted = decision_primary(&decision)
                    .cloned()
                    .unwrap_or_else(|| decision.admitted.clone());
                self.admission_dispose_refusal(device, token, admitted, error)
            }
            Err(_error) => {
                self.admission_abort(device, token);
                AdmissionOutcome::BeginRefused
            }
        }
    }

    fn restore_primary_composed_resources(
        &mut self,
        device: DrmDeviceKey,
        backend_composed: bool,
        resources: Vec<CommitResources>,
    ) -> bool {
        if backend_composed {
            self.scene
                .restore_owner_composed_resources(resources, &mut self.platform.scanout_pools)
        } else if let Some(conductor) = self.admission_conductors.get_mut(&device) {
            conductor.source.restore_composed_resources(resources);
            true
        } else {
            false
        }
    }

    fn admission_dispatch_direct(
        &mut self,
        device: DrmDeviceKey,
        token: AdmissionToken,
        decision: crate::kms::owner::admission::AdmissionDecision,
    ) -> AdmissionOutcome {
        #[cfg(test)]
        self.admission_apply_preparation_hook(device);

        let prepared = match self.managed_prepare_direct_dispatch() {
            Ok(Some(prepared)) => prepared,
            Ok(None) => {
                self.admission_abort(device, token);
                return AdmissionOutcome::PreparationRefused;
            }
            Err(_error) => {
                self.admission_abort(device, token);
                return AdmissionOutcome::PreparationRefused;
            }
        };
        let (desc, context) = match crate::kms::render::direct_owner::description(
            self,
            device,
            &decision,
            &prepared.resources,
        ) {
            Ok(description) => description,
            Err(error) => {
                log::warn!("direct owner description refused: {error}");
                return self.admission_handle_dispatch_failure(
                    device,
                    token,
                    DispatchFailureRoute::Direct {
                        retirement: prepared.retirement,
                    },
                    DispatchFailureResources::Refused {
                        new: vec![prepared.resources],
                    },
                );
            }
        };
        let PreparedDirectDispatch {
            resources: prepared_resources,
            retirement,
        } = prepared;
        let mut resources = Some(prepared_resources);
        let mut retirement = retirement;

        let result = {
            let consumer = &mut self.commit_consumer;
            let Some(service) = self.resource_service.as_mut() else {
                return self.admission_handle_dispatch_failure(
                    device,
                    token,
                    DispatchFailureRoute::Direct {
                        retirement: retirement.take(),
                    },
                    DispatchFailureResources::Refused {
                        new: resources.take().into_iter().collect(),
                    },
                );
            };
            let Some(device_entry) = self
                .platform
                .devices
                .iter_mut()
                .find(|entry| entry.key == device)
            else {
                return self.admission_handle_dispatch_failure(
                    device,
                    token,
                    DispatchFailureRoute::Direct {
                        retirement: retirement.take(),
                    },
                    DispatchFailureResources::Refused {
                        new: resources.take().into_iter().collect(),
                    },
                );
            };
            let Some(owner) = device_entry.owner.as_mut() else {
                return self.admission_handle_dispatch_failure(
                    device,
                    token,
                    DispatchFailureRoute::Direct {
                        retirement: retirement.take(),
                    },
                    DispatchFailureResources::Refused {
                        new: resources.take().into_iter().collect(),
                    },
                );
            };
            owner.begin_with_context_and_fallible_ledger(
                &desc,
                |commit| {
                    let new_resource = resources
                        .take()
                        .ok_or_else(|| (ResourceError::InvalidState, Vec::new(), Vec::new()))?;
                    let new = vec![new_resource.with_commit_id(
                        crate::kms::render::resources::CommitKey::new(device, commit),
                    )];
                    let members = new
                        .iter()
                        .flat_map(|resources| resources.crtcs.iter().copied())
                        .collect::<Vec<GroupMember>>();
                    let old = match consumer.take_current_for_members(&members) {
                        Ok(old) => old,
                        Err(error) => return Err((error, Vec::new(), new)),
                    };
                    register_commit_dependencies(commit, old, new, service)
                },
                context,
            )
        };

        let commit = match result {
            Ok((commit, _events)) => commit,
            Err(FallibleBeginError::Ledger((_error, old, new))) => {
                return self.admission_handle_dispatch_failure(
                    device,
                    token,
                    DispatchFailureRoute::Direct {
                        retirement: retirement.take(),
                    },
                    DispatchFailureResources::Ledger { old, new },
                );
            }
            Err(FallibleBeginError::Cleanup {
                error: (_error, old, new),
                ..
            }) => {
                return self.admission_handle_dispatch_failure(
                    device,
                    token,
                    DispatchFailureRoute::Direct {
                        retirement: retirement.take(),
                    },
                    DispatchFailureResources::Cleanup { old, new },
                );
            }
            Err(FallibleBeginError::Refused { .. }) => {
                return self.admission_handle_dispatch_failure(
                    device,
                    token,
                    DispatchFailureRoute::Direct {
                        retirement: retirement.take(),
                    },
                    DispatchFailureResources::Refused {
                        new: resources.take().into_iter().collect(),
                    },
                );
            }
        };
        if let Some(retirement) = retirement.take() {
            self.commit_consumer.prereserve_retirement(
                crate::kms::render::resources::CommitKey::new(device, commit),
                retirement,
            );
        }
        let send_result = {
            let device_entry = self
                .platform
                .devices
                .iter_mut()
                .find(|entry| entry.key == device)
                .expect("admission device");
            let owner = device_entry.owner.as_mut().expect("admission owner");
            owner.send_on(device_entry.executor.as_mut().expect("admission executor"))
        };
        match send_result {
            Ok(_events) => {
                let outcome = self.admission_confirm(device, token, commit, decision);
                #[cfg(test)]
                if matches!(outcome, AdmissionOutcome::Dispatched(_))
                    && let Some(conductor) = self.admission_conductors.get_mut(&device)
                {
                    conductor.trace.push(AdmissionTraceStep::Dispatched(commit));
                }
                outcome
            }
            Err(error @ DispatchError::Refused { .. }) => {
                let admitted = decision_primary(&decision)
                    .cloned()
                    .unwrap_or_else(|| decision.admitted.clone());
                self.admission_dispose_refusal(device, token, admitted, error)
            }
            Err(_error) => {
                self.admission_abort(device, token);
                AdmissionOutcome::BeginRefused
            }
        }
    }

    fn admission_confirm(
        &mut self,
        device: DrmDeviceKey,
        token: AdmissionToken,
        commit: CommitId,
        decision: AdmissionDecision,
    ) -> AdmissionOutcome {
        let Some(conductor) = self.admission_conductors.get_mut(&device) else {
            if let Some(gate) = self.platform.transport_gate_mut(&device) {
                gate.force_close();
            }
            return AdmissionOutcome::TransportClosed;
        };
        let confirmed = match conductor.admission.confirm(token) {
            Ok(confirmed) => confirmed,
            Err(_error) => {
                if let Some(gate) = self.platform.transport_gate_mut(&device) {
                    gate.force_close();
                }
                return AdmissionOutcome::TransportClosed;
            }
        };
        let bound_violation = self
            .admission_conductors
            .get(&device)
            .is_some_and(|conductor| conductor.admission.bound_violation().is_some());
        if bound_violation && let Some(gate) = self.platform.transport_gate_mut(&device) {
            gate.force_close();
        }

        let maintenance_only = matches!(decision.admitted, Admitted::Maintenance { .. })
            && decision.combined_primary.is_none();
        if !self.admission_record_receipt(device, commit, &decision, maintenance_only) {
            if let Some(gate) = self.platform.transport_gate_mut(&device) {
                gate.force_close();
            }
            return AdmissionOutcome::TransportClosed;
        }

        match decision_primary(&decision) {
            Some(Admitted::Composed { crtc, generation }) => {
                if let Some(conductor) = self.admission_conductors.get_mut(&device)
                    && conductor.composed.get(crtc) == Some(generation)
                {
                    conductor.composed.remove(crtc);
                }
            }
            Some(Admitted::Bundle { members }) => {
                if let Some(conductor) = self.admission_conductors.get_mut(&device) {
                    for member in members {
                        if let Admitted::Composed { crtc, generation } = member
                            && conductor.composed.get(crtc) == Some(generation)
                        {
                            conductor.composed.remove(crtc);
                        }
                    }
                }
            }
            Some(Admitted::Direct { successor }) => {
                if !self.managed_confirm_direct_dispatch(
                    successor.source_generation,
                    crate::kms::render::resources::CommitKey::new(device, commit),
                ) {
                    if let Some(gate) = self.platform.transport_gate_mut(&device) {
                        gate.force_close();
                    }
                    return AdmissionOutcome::TransportClosed;
                }
            }
            Some(Admitted::Topology { .. } | Admitted::Unflip { .. }) => {}
            None | Some(Admitted::Maintenance { .. }) => {}
            Some(Admitted::CursorRecovery { .. }) => {
                return AdmissionOutcome::Unsupported(confirmed.decision.tier);
            }
        }
        if bound_violation {
            return AdmissionOutcome::TransportClosed;
        }
        AdmissionOutcome::Dispatched(confirmed)
    }

    fn admission_record_receipt(
        &mut self,
        device: DrmDeviceKey,
        commit: CommitId,
        decision: &AdmissionDecision,
        maintenance_only: bool,
    ) -> bool {
        let Some(conductor) = self.admission_conductors.get_mut(&device) else {
            return false;
        };
        if decision.carried.is_empty() && !maintenance_only {
            return true;
        }
        if !conductor.receipts.is_empty()
            || decision.carried.iter().any(|carried| {
                conductor.maintenance.submitted.contains_key(&carried.key)
                    || conductor
                        .maintenance
                        .desired
                        .get(&carried.key)
                        .is_none_or(|payload| payload.generation != carried.generation)
            })
        {
            return false;
        }

        let mut payloads = Vec::with_capacity(decision.carried.len());
        for carried in &decision.carried {
            let Some(payload) = conductor.maintenance.desired.remove(&carried.key) else {
                return false;
            };
            payloads.push((carried.key, payload));
        }
        for (key, payload) in payloads {
            conductor.maintenance.submitted.insert(key, payload);
        }
        conductor.receipts.insert(
            commit,
            AdmissionReceipt {
                commit,
                carried: decision.carried.clone(),
                maintenance_only,
            },
        );
        true
    }

    /// Apply a terminal owner outcome to the maintenance payloads carried by
    /// this commit. `Ok(false)` means that this commit is not a maintenance
    /// receipt and the ordinary resource path must handle the event.
    pub(crate) fn admission_handle_terminal(
        &mut self,
        device: DrmDeviceKey,
        commit: CommitId,
        terminal: TerminalState,
    ) -> Result<bool, ()> {
        let Some(receipt) = self
            .admission_conductors
            .get(&device)
            .and_then(|conductor| conductor.receipts.get(&commit))
            .cloned()
        else {
            return Ok(false);
        };

        let Some(conductor) = self.admission_conductors.get(&device) else {
            return Err(());
        };
        let mut keys = BTreeSet::new();
        if receipt
            .carried
            .iter()
            .any(|carried| !keys.insert(carried.key))
        {
            return Err(());
        }
        match terminal {
            TerminalState::Completed => {
                if receipt.carried.iter().any(|carried| {
                    conductor
                        .maintenance
                        .submitted
                        .get(&carried.key)
                        .is_none_or(|payload| payload.generation != carried.generation)
                }) {
                    return Err(());
                }
            }
            TerminalState::FailedBeforeSubmit(FailureCause::IoctlRejected { .. }) => {
                if receipt.carried.iter().any(|carried| {
                    conductor
                        .maintenance
                        .submitted
                        .get(&carried.key)
                        .is_none_or(|payload| payload.generation != carried.generation)
                        || conductor
                            .maintenance
                            .desired
                            .get(&carried.key)
                            .is_some_and(|payload| payload.generation <= carried.generation)
                }) {
                    return Err(());
                }
            }
            TerminalState::CompletionUnknown(_) => {
                if receipt.carried.iter().any(|carried| {
                    conductor
                        .maintenance
                        .submitted
                        .get(&carried.key)
                        .is_none_or(|payload| payload.generation != carried.generation)
                        || conductor.maintenance.dormant.contains_key(&carried.key)
                }) {
                    return Err(());
                }
            }
            TerminalState::FailedBeforeSubmit(FailureCause::NeverDispatched(_)) => {
                return Err(());
            }
        }

        let conductor = self.admission_conductors.get_mut(&device).ok_or(())?;
        match terminal {
            TerminalState::Completed => {
                for carried in &receipt.carried {
                    let payload = conductor
                        .maintenance
                        .submitted
                        .remove(&carried.key)
                        .ok_or(())?;
                    conductor.maintenance.current.insert(carried.key, payload);
                    conductor
                        .admission
                        .note_completed(carried.key, carried.generation);
                }
            }
            TerminalState::FailedBeforeSubmit(FailureCause::IoctlRejected { .. }) => {
                for carried in &receipt.carried {
                    let payload = conductor
                        .maintenance
                        .submitted
                        .remove(&carried.key)
                        .ok_or(())?;
                    if conductor
                        .maintenance
                        .desired
                        .get(&carried.key)
                        .is_none_or(|desired| desired.generation <= carried.generation)
                    {
                        conductor.maintenance.desired.insert(carried.key, payload);
                    }
                    if conductor.admission.reenter(
                        carried.key,
                        carried.generation,
                        carried.ticket,
                        ReentryKind::Rejected,
                    ) == Reentry::Dropped
                    {
                        conductor.maintenance.desired.remove(&carried.key);
                        match carried.key.class {
                            MaintenanceClass::Cursor => {
                                conductor
                                    .admission
                                    .request_cursor_recovery(carried.key.crtc);
                            }
                            MaintenanceClass::Gamma => {
                                conductor.gamma_failures.insert(carried.key.crtc);
                            }
                        }
                    }
                }
            }
            TerminalState::CompletionUnknown(_) => {
                for carried in &receipt.carried {
                    let payload = conductor
                        .maintenance
                        .submitted
                        .remove(&carried.key)
                        .ok_or(())?;
                    conductor.maintenance.dormant.insert(carried.key, payload);
                }
                conductor.recovery_stopped = true;
            }
            TerminalState::FailedBeforeSubmit(FailureCause::NeverDispatched(_)) => {
                return Err(());
            }
        }
        conductor.receipts.remove(&commit);
        Ok(true)
    }

    fn admission_abort(&mut self, device: DrmDeviceKey, token: AdmissionToken) {
        self.admission_conductors
            .get_mut(&device)
            .expect("active admission conductor")
            .admission
            .abort(token)
            .expect("the admission token is consumed exactly once");
    }

    fn admission_dispose_refusal(
        &mut self,
        device: DrmDeviceKey,
        token: AdmissionToken,
        admitted: Admitted,
        error: DispatchError<CommitResources>,
    ) -> AdmissionOutcome {
        let DispatchError::Refused { cause, events } = error else {
            unreachable!("only pre-IPC refusals use refusal disposition")
        };
        let backend_composed = self
            .admission_conductors
            .get(&device)
            .is_some_and(|conductor| conductor.backend_composed);
        let restores_composed = backend_composed
            && matches!(
                admitted,
                Admitted::Composed { .. } | Admitted::Bundle { .. }
            );
        let refused_commit = events.iter().find_map(|event| match event {
            OwnerEvent::ResourcesReleased { commit, .. } => Some(*commit),
            _ => None,
        });
        self.admission_abort(device, token);
        // The owner has already terminalized this pre-IPC refusal. Route its
        // terminal/resource batch through the same backend seam as every other
        // owner milestone; the NeverDispatched arm intentionally does not wake
        // admission before the composed lease is restored below.
        let consumed = self.resource_service.is_some()
            && self.route_owner_event_batch(device, events, std::time::Instant::now());
        let restored = if consumed && restores_composed {
            refused_commit.is_some_and(|commit| {
                let resources = self.commit_consumer.take_rejected_for_commit(
                    crate::kms::render::resources::CommitKey::new(device, commit),
                );
                !resources.is_empty()
                    && self.scene.restore_owner_composed_resources(
                        resources,
                        &mut self.platform.scanout_pools,
                    )
            })
        } else {
            true
        };
        if let Admitted::Direct { successor } = admitted {
            let withdrawn = self
                .admission_conductors
                .get_mut(&device)
                .expect("active admission conductor")
                .admission
                .withdraw_direct(successor.source_generation);
            if withdrawn.is_some() {
                let terminalized = self
                    .managed_terminalize_queued_direct_successor(Some(successor.source_generation));
                debug_assert!(terminalized, "refused direct descriptor names its frame");
            }
        }
        if consumed && restored {
            AdmissionOutcome::SendRefused(cause)
        } else {
            self.platform
                .transport_gate_mut(&device)
                .expect("active admission transport gate")
                .force_close();
            AdmissionOutcome::TransportClosed
        }
    }

    #[cfg(test)]
    fn admission_force_lock_mismatch_if_requested(
        &mut self,
        device: DrmDeviceKey,
        decision: &crate::kms::owner::admission::AdmissionDecision,
    ) {
        let conductor = self
            .admission_conductors
            .get_mut(&device)
            .expect("active admission conductor");
        if !conductor.force_lock_mismatch {
            return;
        }
        conductor.force_lock_mismatch = false;
        if let Admitted::Composed { crtc, generation } = decision.admitted {
            let next = generation.checked_add(1).expect("test generation overflow");
            conductor
                .admission
                .set_composed(crtc, next)
                .expect("test mismatch offer");
            conductor.composed.insert(crtc, next);
        }
    }

    #[cfg(test)]
    fn admission_apply_preparation_hook(&mut self, device: DrmDeviceKey) {
        let hook = self
            .admission_conductors
            .get_mut(&device)
            .expect("active admission conductor")
            .prepare_hook
            .take();
        match hook {
            Some(AdmissionPreparationHook::CloseAdmission) => {
                self.commit_consumer.capacity.close_admission();
            }
            Some(AdmissionPreparationHook::OccupyOrdinaryRetirement) => {
                let reservation = self
                    .commit_consumer
                    .capacity
                    .reserve(crate::kms::render::resources::DirectRole::OrdinaryRetirement)
                    .expect("test retirement reservation");
                let resources =
                    CommitResources::new(Vec::new(), None, None, None, Vec::new(), Vec::new());
                let occupied = self
                    .commit_consumer
                    .capacity
                    .attach(reservation, resources)
                    .expect("test retirement attachment");
                self.commit_consumer.releasing_resources.push(occupied);
            }
            None => {}
        }
    }

    #[cfg(test)]
    pub(crate) fn admission_dispose_refusal_for_tests(
        &mut self,
        device: DrmDeviceKey,
        token: AdmissionToken,
        admitted: Admitted,
        error: DispatchError<CommitResources>,
    ) -> AdmissionOutcome {
        let outcome = self.admission_dispose_refusal(device, token, admitted, error);
        if self.admission_is_active(device) {
            self.managed_publish_deferred_successor_skips_if_no_predecessor();
        }
        outcome
    }
}

fn has_current_direct(backend: &KmsBackend) -> bool {
    backend
        .commit_consumer
        .current_resources
        .iter()
        .any(|resources| {
            resources.direct_role.as_ref().is_some_and(|role| {
                role.role() == crate::kms::render::resources::DirectRole::Current
            })
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn c0_conv_cir_dispatch_failure_fallback_is_fail_closed() {
        // This proves the mapping only — not reachability or end-to-end restoration.
        for (route, kind, restore) in [
            (
                DispatchFailureRouteKind::Primary,
                DispatchFailureKind::Cleanup,
                ResourceRestore::Succeeded,
            ),
            (
                DispatchFailureRouteKind::Direct,
                DispatchFailureKind::Cleanup,
                ResourceRestore::Succeeded,
            ),
        ] {
            let action = dispatch_failure_policy(route, kind, restore);
            assert_eq!(action.outcome, DispatchFailureOutcome::TransportClosed);
            assert!(action.close_gate);
        }
    }
}
