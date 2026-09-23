//! Stage 2c-ii's fixture-level admission conductor.
//!
//! Producers are deliberately represented by [`AdmissionSource`] here. The
//! real producer conversion is stage 2c-iii; this module owns the boundary
//! between that source, A1's pure decider, and the managed 2c-i seams.

use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
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
            device::{DispatchError, FallibleBeginError, OwnerEvent},
            identity::{CommitId, IncarnationId},
            lifecycle::{
                ArbiterInput, CommitProgress, LifecycleAction, LifecycleCommitOutcome,
                LifecycleReceipt, LifecycleReceiptResult, TransitionTag,
            },
            record::{FailureCause, RefusalCause, TerminalState},
        },
        render::{
            backend::{
                DirectEligibility, KmsBackend, PreparedDirectDispatch, effective_refresh_matches,
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
}

struct PendingTopologyValidation {
    tag: TransitionTag<IncarnationId>,
    decision: AdmissionDecision,
    token: Option<AdmissionToken>,
    description: CommitDescription,
    sent: bool,
    cancelled: bool,
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
    topology_commits: BTreeMap<CommitId, TransitionTag<IncarnationId>>,
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
    stale_before_test_only: usize,
    #[cfg(test)]
    stale_before_live_dispatch: usize,
    #[cfg(test)]
    stale_results: usize,
}

impl LifecycleDriver {
    pub(crate) fn new() -> Self {
        Self {
            queue: VecDeque::new(),
            draining: false,
            routing_batch_depth: 0,
            pending_topology_validations: BTreeMap::new(),
            topology_commits: BTreeMap::new(),
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
            stale_before_test_only: 0,
            #[cfg(test)]
            stale_before_live_dispatch: 0,
            #[cfg(test)]
            stale_results: 0,
        }
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
    pub(crate) fn pending_topology_descriptions_for_tests(&self) -> Vec<&CommitDescription> {
        self.pending_topology_validations
            .values()
            .map(|pending| &pending.description)
            .collect()
    }

    pub(crate) fn has_inflight_topology(&self) -> bool {
        !self.pending_topology_validations.is_empty() || !self.topology_commits.is_empty()
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
    pub(crate) fn run_to_completion_test_stats(&self) -> (usize, usize, usize, usize) {
        (
            self.drain_entries,
            self.queued_while_draining,
            self.applied_inputs,
            self.stale_receipts,
        )
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

    fn lifecycle_register_owner_device(
        &mut self,
        device: DrmDeviceKey,
    ) -> Result<(), crate::kms::owner::lifecycle::CoordinatorError> {
        use crate::kms::owner::lifecycle::LifecycleArbiter;

        if self.lifecycle_drivers.contains_key(&device)
            && self.lifecycle_coordinator.device(&device).is_some()
        {
            return Ok(());
        }
        let Some(incarnation) = self
            .platform
            .owner_ref(device)
            .map(|owner| owner.incarnation())
        else {
            return Err(crate::kms::owner::lifecycle::CoordinatorError::UnknownDevice);
        };
        self.admission_conductors
            .entry(device)
            .or_insert_with(|| AdmissionConductor::new(Box::new(LifecycleOnlyAdmissionSource)));
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

        while let Some(work) = self
            .lifecycle_drivers
            .get_mut(&device)
            .and_then(LifecycleDriver::pop)
        {
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
                    let _ = conductor.admission.request_topology(tag);
                }
                self.lifecycle_sync_owner_tag(device, tag);
                self.admission_wake(device, false);
            }
            LifecycleAction::EpochAdvanced(epoch) => {
                let transition = self.lifecycle_current_tag(device).map(|tag| tag.transition);
                if let Some(owner) = self.platform.owner_for(device) {
                    let _ = owner.update_lifecycle_context(epoch, transition);
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

    fn lifecycle_cancel_pre_submit(
        &mut self,
        device: DrmDeviceKey,
        tag: TransitionTag<IncarnationId>,
    ) -> bool {
        let mut success = true;
        if let Some(conductor) = self.admission_conductors.get_mut(&device) {
            conductor.admission.cancel_topology(tag);
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

    fn admission_dispatch_topology(
        &mut self,
        device: DrmDeviceKey,
        token: AdmissionToken,
        decision: AdmissionDecision,
    ) -> AdmissionOutcome {
        let Admitted::Topology { tag } = decision.admitted else {
            self.admission_abort(device, token);
            return AdmissionOutcome::Unsupported(decision.tier);
        };
        if !self.lifecycle_tag_current(device, tag) {
            if let Some(conductor) = self.admission_conductors.get_mut(&device) {
                conductor.admission.cancel_topology(tag);
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
        let validation_commit = {
            let Some(owner) = self.platform.owner_for(device) else {
                self.admission_abort(device, token);
                return AdmissionOutcome::BeginRefused;
            };
            match owner.begin_validation_with_options(
                &description,
                crate::kms::executor::HostCallClass::SeatActiveValidation,
                true,
            ) {
                Ok(commit) => commit,
                Err(_error) => {
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
                    return AdmissionOutcome::BeginRefused;
                }
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
        self.lifecycle_queue_input(
            device,
            ArbiterInput::CommitProgress {
                tag,
                progress: CommitProgress::Submitting,
            },
        );
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
            Some(Ok(_events)) => AdmissionOutcome::NothingAdmissible,
            Some(Err(error @ DispatchError::Refused { .. })) => {
                self.lifecycle_abort_pending_validation(device, validation_commit, true);
                let terminal = match &error {
                    DispatchError::Refused { cause, .. } => {
                        TerminalState::FailedBeforeSubmit(FailureCause::NeverDispatched(*cause))
                    }
                    _ => unreachable!(),
                };
                self.lifecycle_dispose_topology_result(device, validation_commit, tag, terminal);
                AdmissionOutcome::SendRefused(match error {
                    DispatchError::Refused { cause, .. } => cause,
                    _ => unreachable!(),
                })
            }
            Some(Err(_)) | None => {
                self.lifecycle_abort_pending_validation(device, validation_commit, true);
                self.lifecycle_dispose_topology_result(
                    device,
                    validation_commit,
                    tag,
                    TerminalState::FailedBeforeSubmit(FailureCause::NeverDispatched(
                        crate::kms::owner::record::RefusalCause::Reaped,
                    )),
                );
                AdmissionOutcome::BeginRefused
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

    fn lifecycle_finish_topology_validation(
        &mut self,
        device: DrmDeviceKey,
        validation_commit: CommitId,
        outcome: crate::kms::owner::device::ValidationOutcome,
    ) {
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
        let decision = pending.decision.clone();
        let Some(token) = pending.token.take() else {
            return;
        };
        let commit = {
            let Some(owner) = self.platform.owner_for(device) else {
                if let Some(conductor) = self.admission_conductors.get_mut(&device) {
                    let _ = conductor.admission.abort(token);
                }
                return;
            };
            let expected_crtcs = pending
                .description
                .crtc_state
                .iter()
                .filter(|state| state.old_active || state.new_active)
                .map(|state| state.crtc_id)
                .collect::<Vec<_>>();
            let clocks = expected_crtcs
                .iter()
                .filter_map(|&crtc| {
                    owner
                        .clock_key_for_hardware_crtc(crtc)
                        .map(|key| (crtc, key))
                })
                .collect::<BTreeMap<_, _>>();
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
            match owner.begin_validated_with_context(
                &pending.description,
                crate::kms::owner::ledger::Submitted::new(Vec::new(), Vec::new()),
                completion_context,
            ) {
                Ok((commit, _events)) => commit,
                Err(_error) => {
                    let _ = owner.abandon_validation(validation_commit);
                    if let Some(conductor) = self.admission_conductors.get_mut(&device) {
                        let _ = conductor.admission.abort(token);
                    }
                    self.lifecycle_dispose_topology_result(
                        device,
                        validation_commit,
                        tag,
                        TerminalState::FailedBeforeSubmit(FailureCause::IoctlRejected {
                            errno: libc::EINVAL,
                        }),
                    );
                    return;
                }
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
            let _ = self.route_owner_event_batch(device, events, std::time::Instant::now());
            return;
        }
        if let Some(driver) = self.lifecycle_drivers.get_mut(&device) {
            driver.topology_commits.insert(commit, tag);
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
                let _ = self.admission_dispose_refusal(
                    device,
                    token,
                    Admitted::Topology { tag },
                    error,
                );
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

    fn lifecycle_dispose_topology_result(
        &mut self,
        device: DrmDeviceKey,
        commit: CommitId,
        tag: TransitionTag<IncarnationId>,
        terminal: TerminalState,
    ) {
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
            && self
                .lifecycle_drivers
                .get(&device)
                .is_some_and(|driver| driver.pending_topology_validations.contains_key(commit))
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
                let tag = self
                    .lifecycle_drivers
                    .get(&device)
                    .and_then(|driver| driver.topology_commits.get(&commit).copied());
                if let Some(tag) = tag
                    && let Some(driver) = self.lifecycle_drivers.get_mut(&device)
                {
                    driver.enqueue(LifecycleDriverWork::TopologyTerminal {
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
            && self
                .lifecycle_drivers
                .get(&device)
                .and_then(|driver| driver.topology_commits.get(&commit).copied())
                .is_some_and(|tag| !self.lifecycle_tag_current(device, tag));
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

    pub(crate) fn install_admission_conductor(
        &mut self,
        device: DrmDeviceKey,
        source: Box<dyn AdmissionSource>,
    ) {
        self.admission_conductors
            .insert(device, AdmissionConductor::new(source));
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

        self.admission_wake(device, false)
    }

    /// Advance every live device's layout generation after a backend scene
    /// mutation. Layout changes are backend-global, while admission state is
    /// intentionally per DRM device; collect the keys first so a wake can
    /// mutate the conductors without holding a map borrow.
    pub(crate) fn admission_note_layout_change_all_devices(&mut self, reason: &'static str) {
        let devices: Vec<_> = self.admission_conductors.keys().copied().collect();
        for device in devices {
            if matches!(
                self.admission_note_layout_change(device),
                AdmissionOutcome::TransportClosed
            ) {
                log::error!(
                    "admission layout generation overflow or queued successor mismatch for {device:?}: {reason}"
                );
            }
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
                    if backend_composed {
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
                let readiness = if !exit_retirement_vacant {
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
            .is_none_or(|owner| owner.slot().occupant().is_some())
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
            self.admission_dispatch_unflip(device, token, decision)
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
