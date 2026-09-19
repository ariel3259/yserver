//! Stage 2c-ii's fixture-level admission conductor.
//!
//! Producers are deliberately represented by [`AdmissionSource`] here. The
//! real producer conversion is stage 2c-iii; this module owns the boundary
//! between that source, A1's pure decider, and the managed 2c-i seams.

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use crate::{
    kms::{
        owner::{
            admission::{
                Admission, AdmissionDecision, AdmissionError, AdmissionToken, Admitted, Confirmed,
                CrtcId, DirectSuccessor, IntentKey, MaintenanceKey, Readiness, ReadinessSnapshot,
                Tier, WaitReason,
            },
            build::CommitDescription,
            device::{DispatchError, OwnerEvent},
            record::RefusalCause,
        },
        render::{
            backend::{KmsBackend, PreparedDirectDispatch},
            resources::{CommitResources, ResourceError},
        },
    },
    platform::drm::DrmDeviceKey,
};

#[cfg(test)]
use crate::kms::owner::identity::CommitId;

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
    !decision.carried.is_empty()
        || decision.combined_primary.is_some()
        || matches!(
            decision.admitted,
            Admitted::Maintenance { .. }
                | Admitted::Bundle { .. }
                | Admitted::CursorRecovery { .. }
        )
        || matches!(
            decision.tier,
            Tier::AgedMaintenance | Tier::Bundle | Tier::Maintenance
        )
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
    /// Whether the queued direct successor passes current direct eligibility.
    /// 2c-iii replaces this with the real predicate.
    fn direct_eligible(&self, source_generation: u64) -> bool;
}

#[allow(dead_code)]
pub(crate) struct AdmissionConductor {
    pub(crate) admission: Admission,
    pub(crate) source: Box<dyn AdmissionSource>,
    pub(crate) layout_generation: u64,
    pub(crate) next_direct_source_generation: u64,
    pub(crate) composed: BTreeMap<CrtcId, u64>,
    pub(crate) maintenance: MaintenanceStore,
    pub(crate) recovery_stopped: bool,
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
            layout_generation: 0,
            next_direct_source_generation: 1,
            composed: BTreeMap::new(),
            maintenance: MaintenanceStore::default(),
            recovery_stopped: false,
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

    /// Install the fixture-only conductor. Production has no caller until
    /// stage 2c-iii converts the producers.
    pub(crate) fn install_admission_conductor_for_tests(
        &mut self,
        device: DrmDeviceKey,
        source: Box<dyn AdmissionSource>,
    ) {
        self.admission_conductors
            .insert(device, AdmissionConductor::new(source));
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
        self.request_direct_unflip("admission_unflip");
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
        let composed_return_established = self
            .platform
            .outputs
            .iter()
            .enumerate()
            .filter(|(_, output)| output.key.device_key == device)
            .all(|(output_idx, _)| {
                self.platform
                    .retained_composed_framebuffer(output_idx)
                    .is_some()
            });
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
                    conductor
                        .source
                        .producer_readiness(IntentKey::Composed { crtc, generation }),
                );
            }

            if let Some(direct) = conductor.admission.direct() {
                let generation = direct.successor.source_generation;
                let key = IntentKey::Direct {
                    source_generation: generation,
                };
                let eligible = conductor.source.direct_eligible(generation);
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
            snapshot.homogeneous_group = conductor.source.homogeneous_group();
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
        let outcome = if !self.admission_is_active(device) || recovery_stopped {
            AdmissionOutcome::Inert
        } else if self
            .platform
            .owner_ref(device)
            .is_none_or(|owner| owner.slot().occupant().is_some())
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
        if decision_requires_unsupported(&decision) {
            self.admission_abort_unsupported(device, token, &decision)
        } else {
            match decision.admitted.clone() {
                Admitted::Topology { .. } | Admitted::Unflip { .. } => {
                    self.admission_abort(device, token);
                    AdmissionOutcome::Unsupported(decision.tier)
                }
                Admitted::Composed { .. } => {
                    self.admission_dispatch_composed(device, token, decision)
                }
                Admitted::Direct { .. } => self.admission_dispatch_direct(device, token, decision),
                Admitted::Maintenance { .. }
                | Admitted::Bundle { .. }
                | Admitted::CursorRecovery { .. } => {
                    self.admission_abort(device, token);
                    AdmissionOutcome::Unsupported(decision.tier)
                }
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

    fn admission_dispatch_composed(
        &mut self,
        device: DrmDeviceKey,
        token: AdmissionToken,
        decision: crate::kms::owner::admission::AdmissionDecision,
    ) -> AdmissionOutcome {
        let Admitted::Composed { crtc, generation } = decision.admitted.clone() else {
            unreachable!("composed dispatch received a non-composed decision")
        };
        let desc = self
            .admission_conductors
            .get_mut(&device)
            .expect("active admission conductor")
            .source
            .describe(&decision);

        let mut begin_refused = false;
        let commit = {
            let source = &mut self
                .admission_conductors
                .get_mut(&device)
                .expect("active admission conductor")
                .source;
            let consumer = &mut self.commit_consumer;
            let result = {
                let device_entry = self
                    .platform
                    .devices
                    .iter_mut()
                    .find(|entry| entry.key == device)
                    .expect("admission device");
                let owner = device_entry.owner.as_mut().expect("admission owner");
                owner.begin_with_ledger(&desc, |_commit| {
                    let old = consumer.take_current();
                    let new = source.composed_resources(crtc, generation);
                    crate::kms::owner::ledger::Submitted::new(old, new)
                })
            };
            match result {
                Ok((commit, _events)) => Some(commit),
                Err((_error, _builder)) => {
                    begin_refused = true;
                    None
                }
            }
        };

        if begin_refused {
            self.admission_abort(device, token);
            return AdmissionOutcome::BeginRefused;
        }
        let commit = commit.expect("successful begin has a commit id");
        #[cfg(not(test))]
        let _ = commit;
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
                let outcome = self.admission_confirm(device, token, decision.admitted);
                #[cfg(test)]
                if matches!(outcome, AdmissionOutcome::Dispatched(_))
                    && let Some(conductor) = self.admission_conductors.get_mut(&device)
                {
                    conductor.trace.push(AdmissionTraceStep::Dispatched(commit));
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
            Ok(None) | Err(_) => {
                self.admission_abort(device, token);
                return AdmissionOutcome::PreparationRefused;
            }
        };
        let PreparedDirectDispatch {
            resources: prepared_resources,
            retirement,
        } = prepared;
        let mut resources = Some(prepared_resources);
        let mut retirement = retirement;
        let desc = self
            .admission_conductors
            .get_mut(&device)
            .expect("active admission conductor")
            .source
            .describe(&decision);

        let mut begin_refused = false;
        let commit = {
            let consumer = &mut self.commit_consumer;
            let result = {
                let device_entry = self
                    .platform
                    .devices
                    .iter_mut()
                    .find(|entry| entry.key == device)
                    .expect("admission device");
                let owner = device_entry.owner.as_mut().expect("admission owner");
                owner.begin_with_ledger(&desc, |commit| {
                    let old = consumer.take_current();
                    let new = vec![
                        resources
                            .take()
                            .expect("direct builder called once")
                            .with_commit_id(commit),
                    ];
                    crate::kms::owner::ledger::Submitted::new(old, new)
                })
            };
            match result {
                Ok((commit, _events)) => Some(commit),
                Err((_error, _builder)) => {
                    begin_refused = true;
                    None
                }
            }
        };

        if begin_refused {
            let prepared = PreparedDirectDispatch {
                resources: resources.take().expect("refused builder was uncalled"),
                retirement: retirement.take(),
            };
            let _ = self.managed_undo_direct_dispatch(prepared);
            self.admission_abort(device, token);
            return AdmissionOutcome::BeginRefused;
        }

        let commit = commit.expect("successful begin has a commit id");
        if let Some(retirement) = retirement.take() {
            self.commit_consumer
                .prereserve_retirement(commit, retirement);
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
                let outcome = self.admission_confirm(device, token, decision.admitted);
                #[cfg(test)]
                if matches!(outcome, AdmissionOutcome::Dispatched(_))
                    && let Some(conductor) = self.admission_conductors.get_mut(&device)
                {
                    conductor.trace.push(AdmissionTraceStep::Dispatched(commit));
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

    fn admission_confirm(
        &mut self,
        device: DrmDeviceKey,
        token: AdmissionToken,
        admitted: Admitted,
    ) -> AdmissionOutcome {
        let confirmed = self
            .admission_conductors
            .get_mut(&device)
            .expect("active admission conductor")
            .admission
            .confirm(token)
            .expect("the admission token is consumed exactly once");
        match admitted {
            Admitted::Composed { crtc, generation } => {
                let conductor = self
                    .admission_conductors
                    .get_mut(&device)
                    .expect("active admission conductor");
                if conductor.composed.get(&crtc) == Some(&generation) {
                    conductor.composed.remove(&crtc);
                }
            }
            Admitted::Direct { successor } => {
                let confirmed_direct =
                    self.managed_confirm_direct_dispatch(successor.source_generation);
                if !confirmed_direct {
                    if let Some(gate) = self.platform.transport_gate_mut(&device) {
                        gate.force_close();
                    }
                    return AdmissionOutcome::TransportClosed;
                }
            }
            Admitted::Topology { .. }
            | Admitted::Unflip { .. }
            | Admitted::Maintenance { .. }
            | Admitted::Bundle { .. }
            | Admitted::CursorRecovery { .. } => {
                return AdmissionOutcome::Unsupported(confirmed.decision.tier);
            }
        }
        AdmissionOutcome::Dispatched(confirmed)
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
        self.admission_abort(device, token);
        let consumed = self.admission_consume_events(events).is_ok();
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
        if consumed {
            AdmissionOutcome::SendRefused(cause)
        } else {
            self.platform
                .transport_gate_mut(&device)
                .expect("active admission transport gate")
                .force_close();
            AdmissionOutcome::TransportClosed
        }
    }

    fn admission_consume_events(
        &mut self,
        events: Vec<OwnerEvent<CommitResources>>,
    ) -> Result<(), ResourceError> {
        let Some(service) = self.resource_service.as_mut() else {
            return Err(ResourceError::InvalidState);
        };
        let consumer = &mut self.commit_consumer;
        for event in events {
            consumer.consume(event, service)?;
        }
        Ok(())
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
