#![allow(dead_code)]
use std::{
    collections::{BTreeMap, BTreeSet, HashSet},
    fmt,
};

use crate::kms::render::{
    platform::CrtcKey,
    resources::{
        AllocationKey, ObligationId, ResourceService,
        availability::ResourceError,
        capacity::{DirectCapacity, RoleReservation},
        lease::AllocationLease,
        present::{CompletionDisposition, PresentDisposition, PresentKey},
        storage::StorageLease,
        transport::TransportGateHandle,
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
    pub(crate) direct_role: Option<RoleReservation>,
    pub(crate) commit_id: Option<crate::kms::owner::identity::CommitId>,
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
            .field("direct_role", &self.direct_role)
            .field("commit_id", &self.commit_id)
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
            direct_role: None,
            commit_id: None,
        }
    }

    pub(crate) fn with_direct_role(mut self, role: RoleReservation) -> Self {
        self.direct_role = Some(role);
        self
    }

    pub(crate) fn with_commit_id(mut self, commit: crate::kms::owner::identity::CommitId) -> Self {
        self.commit_id = Some(commit);
        self
    }
}

#[derive(Debug, Default)]
pub struct CommitResourceConsumer {
    pub(crate) current_resources: Vec<CommitResources>,
    pub(crate) releasing_resources: Vec<CommitResources>,
    pub(crate) rejected_resources: Vec<CommitResources>,
    pub(crate) hardware_completed_commits: BTreeSet<crate::kms::owner::identity::CommitId>,
    pub(crate) commit_members: BTreeMap<crate::kms::owner::identity::CommitId, Vec<GroupMember>>,
    pub(crate) present_dispositions: BTreeMap<PresentKey, PresentDisposition>,
    pub(crate) reference_crtcs: BTreeMap<PresentKey, u32>,
    pub(crate) capacity: DirectCapacity,
    pub(crate) direct_admission_scheduled: bool,
    pub(crate) gate_handle: Option<TransportGateHandle>,
}

impl CommitResourceConsumer {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn take_current(&mut self) -> Vec<CommitResources> {
        std::mem::take(&mut self.current_resources)
    }

    pub(crate) fn with_gate_handle(mut self, gate: TransportGateHandle) -> Self {
        self.gate_handle = Some(gate);
        self
    }

    pub(crate) fn set_gate_handle(&mut self, gate: TransportGateHandle) {
        self.gate_handle = Some(gate);
    }

    pub(crate) fn record_present_disposition(
        &mut self,
        key: PresentKey,
        disposition: PresentDisposition,
    ) {
        self.present_dispositions.insert(key, disposition);
    }

    pub(crate) fn record_present_disposition_with_reference(
        &mut self,
        key: PresentKey,
        disposition: PresentDisposition,
        reference_crtc: u32,
    ) {
        self.present_dispositions.insert(key, disposition);
        self.reference_crtcs.insert(key, reference_crtc);
    }

    fn freeze_commit_entries(
        &self,
        commit: crate::kms::owner::identity::CommitId,
        service: &mut ResourceService,
    ) {
        for res in &self.releasing_resources {
            if res.commit_id == Some(commit) {
                freeze_resource_allocations(res, service);
            }
        }
        for res in &self.current_resources {
            if res.commit_id == Some(commit) {
                freeze_resource_allocations(res, service);
            }
        }
        for res in &self.rejected_resources {
            if res.commit_id == Some(commit) {
                freeze_resource_allocations(res, service);
            }
        }
    }

    pub(crate) fn consume(
        &mut self,
        event: crate::kms::owner::device::OwnerEvent<CommitResources>,
        service: &mut ResourceService,
    ) -> Result<(), ResourceError> {
        match event {
            crate::kms::owner::device::OwnerEvent::HardwareComplete { commit } => {
                let mut found = false;
                let members = self.commit_members.remove(&commit);
                for res in &mut self.releasing_resources {
                    if res.commit_id == Some(commit) {
                        found = true;
                        let target_members = members
                            .as_ref()
                            .cloned()
                            .unwrap_or_else(|| res.crtcs.clone());
                        discharge_commit_kms_obligations(res, &target_members, service)?;
                    }
                }
                if !found {
                    self.hardware_completed_commits.insert(commit);
                }
                Ok(())
            }
            crate::kms::owner::device::OwnerEvent::CompletionRetired { commit, resources } => {
                let (mut old, new) = resources.into_parts();
                let mut members: Vec<GroupMember> =
                    new.iter().flat_map(|r| r.crtcs.iter().copied()).collect();
                if members.is_empty() {
                    members = old.iter().flat_map(|r| r.crtcs.iter().copied()).collect();
                }
                let hw_completed = self.hardware_completed_commits.remove(&commit);
                for res in &mut old {
                    res.commit_id = Some(commit);
                    if hw_completed {
                        discharge_commit_kms_obligations(res, &members, service)?;
                    }
                }
                if !hw_completed {
                    self.commit_members.insert(commit, members);
                }
                self.releasing_resources.extend(old);
                self.current_resources = new;
                Ok(())
            }
            crate::kms::owner::device::OwnerEvent::ResourcesStillCurrent {
                commit,
                mut resources,
            } => {
                self.hardware_completed_commits.remove(&commit);
                self.commit_members.remove(&commit);
                for res in &mut resources {
                    for (key, obligation_id, _) in res.kms_obligations.drain(..) {
                        let _ = service.cancel(key, obligation_id);
                    }
                }
                self.current_resources = resources;
                Ok(())
            }
            crate::kms::owner::device::OwnerEvent::ResourcesReleased {
                commit,
                mut resources,
            } => {
                self.hardware_completed_commits.remove(&commit);
                self.commit_members.remove(&commit);
                for res in &mut resources {
                    for (key, obligation_id, _) in res.kms_obligations.drain(..) {
                        let _ = service.cancel(key, obligation_id);
                    }
                    res.commit_id = Some(commit);
                }
                self.rejected_resources.extend(resources);
                Ok(())
            }
            crate::kms::owner::device::OwnerEvent::Quarantined { commit } => {
                if let Some(gate) = &self.gate_handle {
                    gate.close_gate();
                }
                self.freeze_commit_entries(commit, service);
                Ok(())
            }
            crate::kms::owner::device::OwnerEvent::Terminal { commit, terminal } => {
                match terminal {
                    crate::kms::owner::record::TerminalState::Completed
                    | crate::kms::owner::record::TerminalState::FailedBeforeSubmit(_) => Ok(()),
                    crate::kms::owner::record::TerminalState::CompletionUnknown(_) => {
                        self.freeze_commit_entries(commit, service);
                        Ok(())
                    }
                }
            }
            crate::kms::owner::device::OwnerEvent::Presented { commit, samples } => {
                for (key, disp) in &mut self.present_dispositions {
                    if key.commit == commit && disp.completion == CompletionDisposition::Pending {
                        disp.completion = CompletionDisposition::Emitted;
                        if let Some(&ref_crtc) = self.reference_crtcs.get(key) {
                            disp.sample = samples
                                .get(&ref_crtc)
                                .copied()
                                .or_else(|| samples.values().next().copied());
                        } else {
                            disp.sample = samples.values().next().copied();
                        }
                    }
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }

    pub(crate) fn on_available(
        &mut self,
        _keys: &[AllocationKey],
        service: &mut ResourceService,
    ) -> Result<(), ResourceError> {
        let mut retained_releasing = Vec::new();
        let mut freed_any = false;
        for mut res in self.releasing_resources.drain(..) {
            if is_resource_releasable(&res, service) {
                if let Some(slot) = res.direct_role.take() {
                    match self.capacity.finish_role(slot) {
                        Ok(()) => {
                            freed_any = true;
                            drop(res);
                        }
                        Err((err, slot)) => {
                            res.direct_role = Some(slot);
                            retained_releasing.push(res);
                            self.capacity.close_admission();
                            return Err(err);
                        }
                    }
                } else {
                    drop(res);
                }
            } else {
                retained_releasing.push(res);
            }
        }
        self.releasing_resources = retained_releasing;

        let mut retained_rejected = Vec::new();
        for mut res in self.rejected_resources.drain(..) {
            if is_resource_releasable(&res, service) {
                if let Some(slot) = res.direct_role.take() {
                    match self.capacity.finish_role(slot) {
                        Ok(()) => {
                            freed_any = true;
                            drop(res);
                        }
                        Err((err, slot)) => {
                            res.direct_role = Some(slot);
                            retained_rejected.push(res);
                            self.capacity.close_admission();
                            return Err(err);
                        }
                    }
                } else {
                    drop(res);
                }
            } else {
                retained_rejected.push(res);
            }
        }
        self.rejected_resources = retained_rejected;

        if freed_any {
            self.direct_admission_scheduled = true;
        }

        Ok(())
    }
}

fn is_resource_releasable(res: &CommitResources, service: &ResourceService) -> bool {
    if !res.kms_obligations.is_empty() {
        return false;
    }
    for alloc in &res.allocations {
        if !service.is_releasable(&alloc.key()) {
            return false;
        }
    }
    if let Some(source) = &res.source
        && !service.is_releasable(&source.allocation.key())
    {
        return false;
    }
    if let Some(fallback) = &res.fallback
        && !service.is_releasable(&fallback.allocation.key())
    {
        return false;
    }
    true
}

fn discharge_commit_kms_obligations(
    res: &mut CommitResources,
    completed_members: &[GroupMember],
    service: &mut ResourceService,
) -> Result<(), ResourceError> {
    let mut matching = Vec::new();
    let mut non_matching = Vec::new();

    for item in res.kms_obligations.drain(..) {
        if completed_members.contains(&item.2) {
            matching.push(item);
        } else {
            non_matching.push(item);
        }
    }

    // Atomic validate-all first (M-4)
    for (key, obligation_id, _) in &matching {
        if let Err(err) = service.validate_proof_target(*key, *obligation_id) {
            matching.extend(non_matching);
            res.kms_obligations = matching;
            return Err(err);
        }
    }

    // Apply all validated proofs
    for (key, obligation_id, _) in matching {
        service.apply_validated_proof(key, obligation_id)?;
    }

    res.kms_obligations = non_matching;
    Ok(())
}

fn freeze_resource_allocations(res: &CommitResources, service: &mut ResourceService) {
    for alloc in &res.allocations {
        let _ = service.freeze(alloc.key());
    }
    if let Some(source) = &res.source {
        let _ = service.freeze(source.allocation.key());
    }
    if let Some(fallback) = &res.fallback {
        let _ = service.freeze(fallback.allocation.key());
    }
    for &(key, _, _) in &res.kms_obligations {
        let _ = service.freeze(key);
    }
}

pub(crate) fn register_commit_dependencies(
    commit: crate::kms::owner::identity::CommitId,
    mut old: Vec<CommitResources>,
    new: Vec<CommitResources>,
    service: &mut ResourceService,
) -> Result<
    crate::kms::owner::ledger::Submitted<CommitResources>,
    (ResourceError, Vec<CommitResources>, Vec<CommitResources>),
> {
    let mut old_members = Vec::new();
    for res in &old {
        old_members.extend_from_slice(&res.crtcs);
    }
    if !GroupMember::validate_unique(&old_members) {
        return Err((ResourceError::InvalidProof, old, new));
    }

    let mut new_members = Vec::new();
    for res in &new {
        new_members.extend_from_slice(&res.crtcs);
    }
    if !GroupMember::validate_unique(&new_members) {
        return Err((ResourceError::InvalidProof, old, new));
    }

    let mut newly_registered: Vec<(AllocationKey, ObligationId)> = Vec::new();

    for old_res in &mut old {
        for member in &old_res.crtcs {
            let new_has_member = new.iter().any(|n| n.crtcs.contains(member));
            if !new_has_member {
                continue;
            }

            for old_alloc in &old_res.allocations {
                let old_key = old_alloc.key();
                let retained_in_new = new.iter().any(|n| {
                    n.crtcs.contains(member) && n.allocations.iter().any(|a| a.key() == old_key)
                });

                if !retained_in_new {
                    match service.register_kms(old_key, commit, *member) {
                        Ok(ob_id) => {
                            newly_registered.push((old_key, ob_id));
                            old_res.kms_obligations.push((old_key, ob_id, *member));
                        }
                        Err(err) => {
                            for (k, ob) in newly_registered {
                                let _ = service.cancel(k, ob);
                            }
                            for r in &mut old {
                                r.kms_obligations.clear();
                            }
                            return Err((err, old, new));
                        }
                    }
                }
            }
        }
    }

    Ok(crate::kms::owner::ledger::Submitted::new(old, new))
}

pub(crate) fn cancel_pre_ipc_commit(
    submitted: crate::kms::owner::ledger::Submitted<CommitResources>,
    service: &mut ResourceService,
) -> (Vec<CommitResources>, Vec<CommitResources>) {
    let (mut old, new) = submitted.into_parts();
    for res in &mut old {
        for (key, obligation_id, _) in res.kms_obligations.drain(..) {
            let _ = service.cancel(key, obligation_id);
        }
    }
    (old, new)
}
