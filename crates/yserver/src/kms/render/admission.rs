//! Stage 2c-ii's fixture-level admission conductor.
//!
//! Producers are deliberately represented by [`AdmissionSource`] here. The
//! real producer conversion is stage 2c-iii; this module owns the boundary
//! between that source, A1's pure decider, and the managed 2c-i seams.

use std::collections::{BTreeMap, BTreeSet};

use crate::{
    kms::{
        owner::{
            admission::{
                Admission, AdmissionError, Admitted, CrtcId, DirectSuccessor, IntentKey, Readiness,
                ReadinessSnapshot, WaitReason,
            },
            build::CommitDescription,
        },
        render::{backend::KmsBackend, resources::CommitResources},
    },
    platform::drm::DrmDeviceKey,
};

/// What 2c-ii cannot observe because producers are converted in 2c-iii.
#[allow(dead_code)]
pub(crate) trait AdmissionSource {
    /// Producer-side readiness: for a composed intent, a reusable buffer and
    /// finished producer waits; for a direct one, its pre-submit source waits.
    fn producer_readiness(&self, key: IntentKey) -> Readiness;
    /// The atomic description of an admitted primary.
    fn describe(&mut self, admitted: &Admitted) -> CommitDescription;
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
        }
    }
}

#[allow(dead_code)]
impl KmsBackend {
    fn admission_is_active(&self, device: DrmDeviceKey) -> bool {
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
