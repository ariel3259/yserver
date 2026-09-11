#![allow(dead_code)]
use std::{
    collections::{BTreeMap, HashSet},
    fmt,
};

use crate::kms::render::{
    platform::CrtcKey,
    resources::{
        AllocationKey, ObligationId, ResourceService,
        availability::ResourceError,
        lease::AllocationLease,
        present::{CompletionDisposition, PresentDisposition, PresentKey},
        storage::StorageLease,
    },
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct GroupMember {
    pub(crate) crtc: CrtcKey,
    pub(crate) topology_generation: u64,
    pub(crate) crtc_epoch: u64,
}

impl GroupMember {
    pub(crate) fn new(crtc: CrtcKey, topology_generation: u64, crtc_epoch: u64) -> Self {
        Self {
            crtc,
            topology_generation,
            crtc_epoch,
        }
    }

    pub(crate) fn validate_unique(members: &[GroupMember]) -> bool {
        let mut set = HashSet::new();
        for m in members {
            if !set.insert(*m) {
                return false;
            }
        }
        true
    }
}

pub struct PresentRelease {
    pub(crate) event: yserver_core::backend::CompletedPresentEvent,
    pub(crate) wake: Option<crate::kms::render::present_completion::PinnedWake>,
}

impl fmt::Debug for PresentRelease {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PresentRelease")
            .field("event", &self.event)
            .field("has_wake", &self.wake.is_some())
            .finish()
    }
}

impl PresentRelease {
    pub(crate) fn new(
        event: yserver_core::backend::CompletedPresentEvent,
        wake: Option<crate::kms::render::present_completion::PinnedWake>,
    ) -> Self {
        Self { event, wake }
    }
}

pub struct CommitResources {
    pub(crate) allocations: Vec<AllocationLease>,
    pub(crate) source: Option<StorageLease>,
    pub(crate) fallback: Option<StorageLease>,
    pub(crate) present: Option<PresentRelease>,
    pub(crate) crtcs: Vec<GroupMember>,
    /// One entry per displaced allocation and member, registered at dispatch;
    /// discharged by this commit's `HardwareComplete` (round-3 M-1).
    pub(crate) kms_obligations: Vec<(AllocationKey, ObligationId, GroupMember)>,
}

impl fmt::Debug for CommitResources {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CommitResources")
            .field("allocations_count", &self.allocations.len())
            .field("has_source", &self.source.is_some())
            .field("has_fallback", &self.fallback.is_some())
            .field("has_present", &self.present.is_some())
            .field("crtcs", &self.crtcs)
            .field("kms_obligations", &self.kms_obligations)
            .finish()
    }
}

impl CommitResources {
    pub(crate) fn new(
        allocations: Vec<AllocationLease>,
        source: Option<StorageLease>,
        fallback: Option<StorageLease>,
        present: Option<PresentRelease>,
        crtcs: Vec<GroupMember>,
        kms_obligations: Vec<(AllocationKey, ObligationId, GroupMember)>,
    ) -> Self {
        Self {
            allocations,
            source,
            fallback,
            present,
            crtcs,
            kms_obligations,
        }
    }
}

type InFlightCorrelation = (
    Vec<GroupMember>,
    Vec<(AllocationKey, ObligationId, GroupMember)>,
);

#[derive(Debug, Default)]
pub struct CommitResourceConsumer {
    pub(crate) current_resources: Vec<CommitResources>,
    pub(crate) releasing_resources: Vec<CommitResources>,
    pub(crate) in_flight: BTreeMap<crate::kms::owner::identity::CommitId, InFlightCorrelation>,
    pub(crate) present_dispositions: BTreeMap<PresentKey, PresentDisposition>,
}

impl CommitResourceConsumer {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn correlate_commit(
        &mut self,
        commit: crate::kms::owner::identity::CommitId,
        members: Vec<GroupMember>,
        obligations: Vec<(AllocationKey, ObligationId, GroupMember)>,
    ) {
        self.in_flight.insert(commit, (members, obligations));
    }

    pub(crate) fn record_present_disposition(
        &mut self,
        key: PresentKey,
        disposition: PresentDisposition,
    ) {
        self.present_dispositions.insert(key, disposition);
    }

    pub(crate) fn consume(
        &mut self,
        event: crate::kms::owner::device::OwnerEvent<CommitResources>,
        service: &mut ResourceService,
    ) -> Result<(), ResourceError> {
        match event {
            crate::kms::owner::device::OwnerEvent::HardwareComplete { commit } => {
                // Round-3 M-1: KMS proof reaches the service via HardwareComplete.
                // Discharges obligations ONLY for displaced allocations in the `old` set matching
                // GroupMember in the completed commit's membership. Nothing is discharged for `new`.
                if let Some((members, mut obligations)) = self.in_flight.remove(&commit) {
                    let mut remaining = Vec::new();
                    for (key, obligation_id, member) in obligations.drain(..) {
                        if members.contains(&member) {
                            service.apply_validated_proof(key, obligation_id)?;
                        } else {
                            remaining.push((key, obligation_id, member));
                        }
                    }
                    if !remaining.is_empty() {
                        self.in_flight.insert(commit, (members, remaining));
                    }
                }
                for res in &mut self.releasing_resources {
                    let mut remaining = Vec::new();
                    for (key, obligation_id, member) in res.kms_obligations.drain(..) {
                        let member_matched = res.crtcs.is_empty() || res.crtcs.contains(&member);
                        if member_matched {
                            service.apply_validated_proof(key, obligation_id)?;
                        } else {
                            remaining.push((key, obligation_id, member));
                        }
                    }
                    res.kms_obligations = remaining;
                }
                Ok(())
            }
            crate::kms::owner::device::OwnerEvent::CompletionRetired {
                commit: _,
                resources,
            } => {
                let (old, new) = resources.into_parts();
                self.releasing_resources.extend(old);
                self.current_resources = new;
                Ok(())
            }
            crate::kms::owner::device::OwnerEvent::ResourcesStillCurrent { commit, resources } => {
                // Rejection: cancel — do not discharge — this commit's KmsRelease registrations
                if let Some((_, obligations)) = self.in_flight.remove(&commit) {
                    for (key, obligation_id, _) in obligations {
                        let _ = service.cancel(key, obligation_id);
                    }
                }
                for res in &resources {
                    for &(key, obligation_id, _) in &res.kms_obligations {
                        let _ = service.cancel(key, obligation_id);
                    }
                }
                self.current_resources = resources;
                Ok(())
            }
            crate::kms::owner::device::OwnerEvent::ResourcesReleased { commit, resources } => {
                // Remove only the rejected/never-current KMS obligation justified by this outcome
                if let Some((_, obligations)) = self.in_flight.remove(&commit) {
                    for (key, obligation_id, _) in obligations {
                        let _ = service.cancel(key, obligation_id);
                    }
                }
                for res in &resources {
                    for &(key, obligation_id, _) in &res.kms_obligations {
                        let _ = service.cancel(key, obligation_id);
                    }
                }
                drop(resources);
                Ok(())
            }
            crate::kms::owner::device::OwnerEvent::Quarantined { commit } => {
                if let Some((_, obligations)) = self.in_flight.get(&commit) {
                    for &(key, _, _) in obligations {
                        let _ = service.freeze(key);
                    }
                }
                for res in &self.current_resources {
                    for &(key, _, _) in &res.kms_obligations {
                        let _ = service.freeze(key);
                    }
                }
                for res in &self.releasing_resources {
                    for &(key, _, _) in &res.kms_obligations {
                        let _ = service.freeze(key);
                    }
                }
                Ok(())
            }
            crate::kms::owner::device::OwnerEvent::Terminal { commit, .. } => {
                if let Some((_, obligations)) = self.in_flight.get(&commit) {
                    for &(key, _, _) in obligations {
                        let _ = service.freeze(key);
                    }
                }
                for res in &self.current_resources {
                    for &(key, _, _) in &res.kms_obligations {
                        let _ = service.freeze(key);
                    }
                }
                for res in &self.releasing_resources {
                    for &(key, _, _) in &res.kms_obligations {
                        let _ = service.freeze(key);
                    }
                }
                Ok(())
            }
            crate::kms::owner::device::OwnerEvent::Presented { commit, .. } => {
                // Consume completion disposition using selected/reference CRTC sample;
                // do not release source, fallback or wake.
                for (key, disp) in &mut self.present_dispositions {
                    if key.commit == commit && disp.completion == CompletionDisposition::Pending {
                        disp.completion = CompletionDisposition::Emitted;
                    }
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }
}
