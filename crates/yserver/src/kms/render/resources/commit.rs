#![allow(dead_code)]
use std::{
    collections::{BTreeMap, BTreeSet, HashSet},
    fmt,
};

use crate::{kms::owner::identity::CommitId, platform::drm::DrmDeviceKey};

use crate::kms::render::{
    platform::CrtcKey,
    resources::{
        AllocationKey, ObligationId, ResourceService,
        availability::ResourceError,
        capacity::{DirectCapacity, DirectRole, RoleReservation},
        drm_cleanup::CleanupCharge,
        lease::AllocationLease,
        present::{CompletionDisposition, PresentDisposition, PresentKey, ReleaseDisposition},
        storage::StorageLease,
        transport::TransportGateHandle,
    },
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct CommitKey {
    pub(crate) device: DrmDeviceKey,
    pub(crate) commit: CommitId,
}

impl CommitKey {
    pub(crate) const fn new(device: DrmDeviceKey, commit: CommitId) -> Self {
        Self { device, commit }
    }
}

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

/// One allocation's KMS-release registration for an owner commit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct KmsReleaseObligation {
    pub(crate) allocation: AllocationKey,
    pub(crate) obligation: ObligationId,
    pub(crate) member: GroupMember,
    pub(crate) commit: CommitId,
}

/// Proof that an inactive CRTC remained dark while a later commit displaced
/// the old framebuffer. The issuer checks the installed-power chain before
/// constructing this value; its fields are deliberately limited to the
/// evidence named by design §4.2.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DarkCrtcDisplacement {
    pub(crate) off_commit: CommitId,
    pub(crate) crtc: CrtcKey,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KmsReleaseProof {
    CompletionRetired {
        through_commit: CommitId,
        crtc: CrtcKey,
    },
    DarkCrtcDisplacement {
        through_commit: CommitId,
        proof: DarkCrtcDisplacement,
    },
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
    pub(crate) commit_id: Option<CommitKey>,
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

    pub(crate) fn with_commit_id(mut self, commit: CommitKey) -> Self {
        self.commit_id = Some(commit);
        self
    }
}

#[derive(Debug, Default)]
pub struct CommitResourceConsumer {
    pub(crate) current_resources: Vec<CommitResources>,
    pub(crate) releasing_resources: Vec<CommitResources>,
    pub(crate) rejected_resources: Vec<CommitResources>,
    pub(crate) hardware_completed_commits: BTreeSet<CommitKey>,
    pub(crate) commit_members: BTreeMap<CommitKey, Vec<GroupMember>>,
    pub(crate) present_dispositions: BTreeMap<PresentKey, PresentDisposition>,
    pub(crate) reference_crtcs: BTreeMap<PresentKey, u32>,
    pub(crate) capacity: DirectCapacity,
    pub(crate) direct_admission_scheduled: bool,
    pub(crate) gate_handle: Option<TransportGateHandle>,
    pub(crate) released_presents: Vec<PresentRelease>,
    pub(crate) reserved_retirements: BTreeMap<CommitKey, RoleReservation>,
    pub(crate) direct_cleanup_charges: BTreeMap<AllocationKey, CleanupCharge>,
}

impl CommitResourceConsumer {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn prereserve_retirement(&mut self, commit: CommitKey, slot: RoleReservation) {
        self.reserved_retirements.insert(commit, slot);
    }

    pub(crate) fn take_current(&mut self) -> Vec<CommitResources> {
        std::mem::take(&mut self.current_resources)
    }

    /// Remove only the current entries covered by a commit. A resource entry
    /// is the unit of ownership, so seeing only part of one entry covered is
    /// an invalid shape rather than permission to split the entry.
    pub(crate) fn take_current_for_members(
        &mut self,
        members: &[GroupMember],
    ) -> Result<Vec<CommitResources>, ResourceError> {
        if !GroupMember::validate_unique(members) {
            return Err(ResourceError::InvalidProof);
        }

        let covered = |member: &GroupMember| members.contains(member);
        if self.current_resources.iter().any(|resources| {
            let intersects = resources.crtcs.iter().any(covered);
            intersects && resources.crtcs.iter().any(|member| !covered(member))
        }) {
            return Err(ResourceError::InvalidProof);
        }

        let current = std::mem::take(&mut self.current_resources);
        let mut selected = Vec::new();
        let mut retained = Vec::with_capacity(current.len());
        for resources in current {
            if resources.crtcs.iter().any(covered) {
                selected.push(resources);
            } else {
                retained.push(resources);
            }
        }
        self.current_resources = retained;
        Ok(selected)
    }

    pub(crate) fn take_released_presents(&mut self) -> Vec<PresentRelease> {
        std::mem::take(&mut self.released_presents)
    }

    /// Remove the new-state resources returned by a pre-IPC owner refusal.
    /// The owner tags every released entry with the record's commit id, so a
    /// composed caller can hand exactly that state back to its prepared
    /// generation without touching another rejected commit.
    pub(crate) fn take_rejected_for_commit(&mut self, commit: CommitKey) -> Vec<CommitResources> {
        let rejected = std::mem::take(&mut self.rejected_resources);
        let mut matching = Vec::new();
        let mut retained = Vec::with_capacity(rejected.len());
        for resources in rejected {
            if resources.commit_id == Some(commit) {
                matching.push(resources);
            } else {
                retained.push(resources);
            }
        }
        self.rejected_resources = retained;
        matching
    }

    pub(crate) fn take_direct_cleanup_charge(
        &mut self,
        key: AllocationKey,
    ) -> Option<CleanupCharge> {
        self.direct_cleanup_charges.remove(&key)
    }

    pub(crate) fn restore_direct_cleanup_charge(
        &mut self,
        key: AllocationKey,
        charge: CleanupCharge,
    ) {
        let previous = self.direct_cleanup_charges.insert(key, charge);
        if previous.is_some() {
            self.capacity.close_admission();
        }
    }

    pub(crate) fn present_disposition(&self, key: &PresentKey) -> Option<PresentDisposition> {
        self.present_dispositions.get(key).copied()
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

    fn freeze_commit_entries(&self, commit: CommitKey, service: &mut ResourceService) {
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
        commit_key: CommitKey,
        event: crate::kms::owner::device::OwnerEvent<CommitResources>,
        service: &mut ResourceService,
    ) -> Result<(), ResourceError> {
        match event {
            crate::kms::owner::device::OwnerEvent::HardwareComplete { commit: _ } => {
                let mut found = false;
                let members = self.commit_members.remove(&commit_key);
                for res in &mut self.releasing_resources {
                    if res.commit_id == Some(commit_key) {
                        found = true;
                        let target_members = members
                            .as_ref()
                            .cloned()
                            .unwrap_or_else(|| res.crtcs.clone());
                        discharge_commit_kms_obligations(res, &target_members, service)?;
                    }
                }
                if !found {
                    self.hardware_completed_commits.insert(commit_key);
                }
                Ok(())
            }
            crate::kms::owner::device::OwnerEvent::CompletionRetired {
                commit: _,
                resources,
            } => {
                let (mut old, mut new) = resources.into_parts();
                let mut members: Vec<GroupMember> =
                    new.iter().flat_map(|r| r.crtcs.iter().copied()).collect();
                if members.is_empty() {
                    members = old.iter().flat_map(|r| r.crtcs.iter().copied()).collect();
                }
                let hw_completed = self.hardware_completed_commits.remove(&commit_key);
                for res in &mut old {
                    res.commit_id = Some(commit_key);
                    if hw_completed {
                        discharge_commit_kms_obligations(res, &members, service)?;
                    }
                    // M-7: old current moves into pre-reserved retirement role
                    if let Some(ref mut role) = res.direct_role
                        && role.role == DirectRole::Current
                    {
                        if let Some(reserved) = self.reserved_retirements.remove(&commit_key) {
                            if let Err((err, recovered)) =
                                self.capacity.move_into_reserved(role, reserved)
                            {
                                self.reserved_retirements.insert(commit_key, recovered);
                                self.capacity.close_admission();
                                return Err(err);
                            }
                        } else if self.capacity.is_vacant(DirectRole::OrdinaryRetirement)
                            && let Err(err) = self
                                .capacity
                                .move_role(role, DirectRole::OrdinaryRetirement)
                        {
                            self.capacity.close_admission();
                            return Err(err);
                        }
                    }
                }
                // A pre-reserved ordinary retirement is only consumed by an
                // old direct Current role. Defence in depth: if this commit's
                // old state had no such role, cancel the unused reservation
                // in the same retirement event so it cannot leak capacity.
                if let Some(reserved) = self.reserved_retirements.remove(&commit_key)
                    && let Err((err, recovered)) = self.capacity.cancel_reservation(reserved)
                {
                    self.reserved_retirements.insert(commit_key, recovered);
                    self.capacity.close_admission();
                    return Err(err);
                }
                if !hw_completed {
                    self.commit_members.insert(commit_key, members);
                }
                // M-7: new submitted moves into Current
                for res in &mut new {
                    if let Some(ref mut role) = res.direct_role
                        && role.role == DirectRole::Submitted
                        && let Err(err) = self.capacity.move_role(role, DirectRole::Current)
                    {
                        self.capacity.close_admission();
                        return Err(err);
                    }
                }
                self.releasing_resources.extend(old);
                self.current_resources.extend(new);
                Ok(())
            }
            crate::kms::owner::device::OwnerEvent::ResourcesStillCurrent {
                commit: _,
                mut resources,
            } => {
                self.hardware_completed_commits.remove(&commit_key);
                self.commit_members.remove(&commit_key);
                if let Some(reserved) = self.reserved_retirements.remove(&commit_key) {
                    let _ = self.capacity.cancel_reservation(reserved);
                }
                for res in &mut resources {
                    if let Some(role) = res.direct_role.as_mut()
                        && role.role() == DirectRole::ExitRetirement
                        && let Err(error) = self.capacity.move_role(role, DirectRole::Current)
                    {
                        self.capacity.close_admission();
                        return Err(error);
                    }
                    for (key, obligation_id, _) in res.kms_obligations.drain(..) {
                        let _ = service.cancel(key, obligation_id);
                    }
                }
                self.current_resources.extend(resources);
                Ok(())
            }
            crate::kms::owner::device::OwnerEvent::ResourcesReleased {
                commit: _,
                mut resources,
            } => {
                self.hardware_completed_commits.remove(&commit_key);
                self.commit_members.remove(&commit_key);
                if let Some(reserved) = self.reserved_retirements.remove(&commit_key) {
                    let _ = self.capacity.cancel_reservation(reserved);
                }
                for res in &mut resources {
                    for (key, obligation_id, _) in res.kms_obligations.drain(..) {
                        let _ = service.cancel(key, obligation_id);
                    }
                    res.commit_id = Some(commit_key);
                }
                self.rejected_resources.extend(resources);
                Ok(())
            }
            crate::kms::owner::device::OwnerEvent::Quarantined { commit: _ } => {
                if let Some(gate) = &self.gate_handle {
                    gate.close_gate();
                }
                self.freeze_commit_entries(commit_key, service);
                Ok(())
            }
            crate::kms::owner::device::OwnerEvent::Terminal {
                commit: _,
                terminal,
            } => match terminal {
                crate::kms::owner::record::TerminalState::Completed => Ok(()),
                crate::kms::owner::record::TerminalState::FailedBeforeSubmit(_) => {
                    for (key, disp) in &mut self.present_dispositions {
                        if key.device == commit_key.device
                            && key.commit == commit_key.commit
                            && disp.completion == CompletionDisposition::Pending
                        {
                            disp.completion = CompletionDisposition::Suppressed;
                            disp.release = ReleaseDisposition::Retained;
                        }
                    }
                    Ok(())
                }
                crate::kms::owner::record::TerminalState::CompletionUnknown(_) => {
                    self.freeze_commit_entries(commit_key, service);
                    Ok(())
                }
            },
            crate::kms::owner::device::OwnerEvent::Presented { commit: _, samples } => {
                for (key, disp) in &mut self.present_dispositions {
                    if key.device == commit_key.device
                        && key.commit == commit_key.commit
                        && disp.completion == CompletionDisposition::Pending
                    {
                        disp.completion = CompletionDisposition::Emitted;
                        disp.release = ReleaseDisposition::Retained;
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
        let mut freed_any = false;
        let mut transition_error = None;

        let releasing = std::mem::take(&mut self.releasing_resources);
        let mut retained_releasing = Vec::with_capacity(releasing.len());
        let mut releasing_iter = releasing.into_iter();

        while let Some(mut res) = releasing_iter.next() {
            if is_resource_releasable(&res, service) {
                if self.defer_final_direct_role(&mut res, service) {
                    self.release_present(&mut res);
                    drop(res);
                } else if let Some(slot) = res.direct_role.take() {
                    match self.capacity.finish_role(slot) {
                        Ok(()) => {
                            freed_any = true;
                            self.release_present(&mut res);
                            drop(res);
                        }
                        Err((err, slot)) => {
                            res.direct_role = Some(slot);
                            retained_releasing.push(res);
                            retained_releasing.extend(releasing_iter);
                            self.capacity.close_admission();
                            transition_error = Some(err);
                            break;
                        }
                    }
                } else {
                    self.release_present(&mut res);
                    drop(res);
                }
            } else {
                retained_releasing.push(res);
            }
        }
        self.releasing_resources = retained_releasing;
        if let Some(err) = transition_error {
            return Err(err);
        }

        let rejected = std::mem::take(&mut self.rejected_resources);
        let mut retained_rejected = Vec::with_capacity(rejected.len());
        let mut rejected_iter = rejected.into_iter();

        while let Some(mut res) = rejected_iter.next() {
            if is_resource_releasable(&res, service) {
                if self.defer_final_direct_role(&mut res, service) {
                    self.release_present(&mut res);
                    drop(res);
                } else if let Some(slot) = res.direct_role.take() {
                    match self.capacity.finish_role(slot) {
                        Ok(()) => {
                            freed_any = true;
                            self.release_present(&mut res);
                            drop(res);
                        }
                        Err((err, slot)) => {
                            res.direct_role = Some(slot);
                            retained_rejected.push(res);
                            retained_rejected.extend(rejected_iter);
                            self.capacity.close_admission();
                            transition_error = Some(err);
                            break;
                        }
                    }
                } else {
                    self.release_present(&mut res);
                    drop(res);
                }
            } else {
                retained_rejected.push(res);
            }
        }
        self.rejected_resources = retained_rejected;
        if let Some(err) = transition_error {
            return Err(err);
        }

        if freed_any {
            self.direct_admission_scheduled = true;
        }
        Ok(())
    }

    fn defer_final_direct_role(
        &mut self,
        res: &mut CommitResources,
        service: &ResourceService,
    ) -> bool {
        let Some(key) = res.allocations.iter().find_map(|lease| {
            service
                .direct_framebuffer_is_last_lease(lease)
                .then_some(lease.key())
        }) else {
            return false;
        };
        let Some(slot) = res.direct_role.take() else {
            return false;
        };
        let previous = self
            .direct_cleanup_charges
            .insert(key, CleanupCharge::FinalRole(slot));
        if previous.is_some() {
            self.capacity.close_admission();
        }
        true
    }

    fn release_present(&mut self, res: &mut CommitResources) {
        if let Some(present_rel) = res.present.take() {
            let pid = present_rel.event.present_id;
            for (key, disposition) in &mut self.present_dispositions {
                if key.present_id == pid
                    && res.commit_id.is_none_or(|commit| {
                        key.device == commit.device && key.commit == commit.commit
                    })
                {
                    disposition.release = ReleaseDisposition::Released;
                }
            }
            self.released_presents.push(present_rel);
        }
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

pub(crate) fn register_kms_displacements(
    commit: CommitId,
    member: GroupMember,
    allocations: &[AllocationKey],
    service: &mut ResourceService,
) -> Result<Vec<KmsReleaseObligation>, ResourceError> {
    let mut unique = HashSet::new();
    if allocations.iter().any(|key| !unique.insert(*key)) {
        return Err(ResourceError::InvalidProof);
    }

    let mut registrations = Vec::with_capacity(allocations.len());
    for &allocation in allocations {
        match service.register_kms(allocation, commit, member) {
            Ok(obligation) => registrations.push(KmsReleaseObligation {
                allocation,
                obligation,
                member,
                commit,
            }),
            Err(error) => {
                for registration in registrations.drain(..) {
                    let _ = service.cancel(registration.allocation, registration.obligation);
                }
                return Err(error);
            }
        }
    }
    Ok(registrations)
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
