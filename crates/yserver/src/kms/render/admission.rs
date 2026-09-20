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
                Admission, AdmissionDecision, AdmissionError, AdmissionToken, Admitted,
                CarriedMaintenance, Confirmed, CrtcId, DirectSuccessor, IntentKey,
                MaintenanceClass, MaintenanceKey, Readiness, ReadinessSnapshot, Reentry,
                ReentryKind, Tier, WaitReason,
            },
            build::CommitDescription,
            device::{DispatchError, FallibleBeginError, OwnerEvent},
            identity::CommitId,
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

#[derive(Clone, Copy, Debug)]
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
    matches!(
        decision.admitted,
        Admitted::Topology { .. } | Admitted::Unflip { .. } | Admitted::CursorRecovery { .. }
    )
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
        let backend_composed = self
            .admission_conductors
            .get(&device)
            .is_some_and(|conductor| conductor.backend_composed);
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
        let direct_eligibility = self
            .admission_conductors
            .get(&device)
            .and_then(|conductor| conductor.admission.direct())
            .map(|direct| {
                self.direct_successor_eligibility(device, direct.successor.source_generation)
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
                    if !scene.install_owner_damage_transaction(commit, &transaction_specs) {
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
        let PreparedDirectDispatch {
            resources: prepared_resources,
            retirement,
        } = prepared;
        let mut resources = Some(prepared_resources);
        let mut retirement = retirement;
        let desc = match crate::kms::render::direct_owner::description(self, device, &decision) {
            Ok(desc) => desc,
            Err(error) => {
                log::warn!("direct owner description refused: {error}");
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

        let direct_crtcs = match decision_primary(&decision) {
            Some(Admitted::Direct { successor }) => Some(successor.crtcs.clone()),
            _ => None,
        };
        let result = {
            let consumer = &mut self.commit_consumer;
            let Some(service) = self.resource_service.as_mut() else {
                self.admission_abort(device, token);
                return AdmissionOutcome::BeginRefused;
            };
            let device_entry = self
                .platform
                .devices
                .iter_mut()
                .find(|entry| entry.key == device)
                .expect("admission device");
            let owner = device_entry.owner.as_mut().expect("admission owner");
            owner.begin_with_fallible_ledger(&desc, |commit| {
                let mut new_resource = resources
                    .take()
                    .ok_or_else(|| (ResourceError::InvalidState, Vec::new(), Vec::new()))?;
                if new_resource.crtcs.is_empty()
                    && let Some(direct_crtcs) = direct_crtcs.as_ref()
                {
                    new_resource.crtcs = consumer
                        .current_resources
                        .iter()
                        .flat_map(|resources| resources.crtcs.iter().copied())
                        .filter(|member| direct_crtcs.contains(&u32::from(member.crtc.crtc)))
                        .collect();
                }
                let new = vec![new_resource.with_commit_id(commit)];
                let members = new
                    .iter()
                    .flat_map(|resources| resources.crtcs.iter().copied())
                    .collect::<Vec<GroupMember>>();
                let old = match consumer.take_current_for_members(&members) {
                    Ok(old) => old,
                    Err(error) => return Err((error, Vec::new(), new)),
                };
                register_commit_dependencies(commit, old, new, service)
            })
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
                if !self.managed_confirm_direct_dispatch(successor.source_generation) {
                    if let Some(gate) = self.platform.transport_gate_mut(&device) {
                        gate.force_close();
                    }
                    return AdmissionOutcome::TransportClosed;
                }
            }
            None | Some(Admitted::Maintenance { .. }) => {}
            Some(Admitted::Topology { .. })
            | Some(Admitted::Unflip { .. })
            | Some(Admitted::CursorRecovery { .. }) => {
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
                let resources = self.commit_consumer.take_rejected_for_commit(commit);
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
