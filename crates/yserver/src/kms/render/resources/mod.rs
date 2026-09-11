pub(crate) mod availability;
pub(crate) mod capacity;
pub(crate) mod commit;
pub(crate) mod completion;
pub(crate) mod drm_cleanup;
pub(crate) mod gpu;
pub(crate) mod handoff;
pub(crate) mod lease;
pub(crate) mod present;
pub(crate) mod scanout;
pub(crate) mod storage;
pub(crate) mod transport;

#[cfg(test)]
pub(crate) mod tests;

use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
    rc::Rc,
    time::Instant,
};

use crate::{kms::owner::identity::IncarnationId, platform::drm::DrmDeviceKey};

pub(crate) use availability::{
    AllocationEntry, AllocationKey, ObligationId, ObligationKind, ResourceError, UseId, UseKind,
    can_destroy,
};
#[allow(unused_imports)]
pub(crate) use capacity::{DirectCapacity, DirectRole, RoleReservation, RoleState};
#[allow(unused_imports)]
pub use commit::{CommitResourceConsumer, CommitResources, GroupMember, PresentRelease};
#[allow(unused_imports)]
pub(crate) use completion::{ResourceConsumer, ResourceWaiter, WaiterRegistry};
#[allow(unused_imports)]
pub(crate) use drm_cleanup::{
    CleanupIo, DeviceCleanupIo, DirectFramebufferAllocation, DrmCleanupRegistry, DrmCleanupRight,
    FakeFamilyInventory, FileFamilyClosed, GemOwner, RightState,
};
#[allow(unused_imports)]
pub(crate) use gpu::{CoreRetirementBatch, GpuObligation, ReadObligation, ValidatedGpuBatch};
#[allow(unused_imports)]
pub(crate) use handoff::{
    CompletionIngress, DeviceBarrier, HandoffRouter, IncarnationBundle, KmsDisposition,
    RecipientSlot, RetainingSupervisor, TeardownRelease,
};
pub(crate) use lease::AllocationLease;
#[allow(unused_imports)]
pub use present::{CompletionDisposition, PresentDisposition, PresentKey, ReleaseDisposition};
#[allow(unused_imports)]
pub(crate) use scanout::{
    CopiedSourceAllocation, FileOwnedBacking, ManagedScanoutToken, ScanoutAllocation, SharedBacking,
};
#[allow(unused_imports)]
pub(crate) use storage::{
    PixelIdentity, StorageAccessError, StorageAllocation, StorageBacking, StorageLease,
};
#[allow(unused_imports)]
pub(crate) use transport::{
    HandoverPermit, OwnerWriteGrant, RecipientReservation, TransportGate, TransportState,
    WriterClass, WriterCoverageProof,
};

#[allow(dead_code, clippy::large_enum_variant)]
#[derive(Debug)]
pub(crate) enum AllocationPayload {
    #[cfg(test)]
    Spy(tests::SpyAllocation),
    DirectFramebuffer(DirectFramebufferAllocation),
    Storage(storage::StorageAllocation),
    Scanout(scanout::ScanoutAllocation),
    CopiedSource(scanout::CopiedSourceAllocation),
    #[doc(hidden)]
    Unused(std::convert::Infallible),
}

#[allow(dead_code)]
#[derive(Debug)]
pub(crate) struct ResourceService {
    device: DrmDeviceKey,
    incarnation: IncarnationId,
    next_generation: u64,
    next_use_id: u64,
    next_obligation_id: u64,
    exhausted: bool,
    entries: BTreeMap<AllocationKey, Rc<AllocationEntry>>,
    dirty_entries: Rc<RefCell<BTreeSet<AllocationKey>>>,
    pending_batches: Vec<CoreRetirementBatch>,
    quarantined_batches: Vec<(CoreRetirementBatch, ResourceError)>,
    seat_active: bool,
    waiters: WaiterRegistry,
    serviced_elapsed: std::time::Duration,
    last_serviced: Option<Instant>,
    max_serviced_duration: std::time::Duration,
}

#[allow(dead_code)]
impl ResourceService {
    pub(crate) fn new(device: DrmDeviceKey, incarnation: IncarnationId) -> Self {
        Self {
            device,
            incarnation,
            next_generation: 1,
            next_use_id: 1,
            next_obligation_id: 1,
            exhausted: false,
            entries: BTreeMap::new(),
            dirty_entries: Rc::new(RefCell::new(BTreeSet::new())),
            pending_batches: Vec::new(),
            quarantined_batches: Vec::new(),
            seat_active: true,
            waiters: WaiterRegistry::new(),
            serviced_elapsed: std::time::Duration::ZERO,
            last_serviced: None,
            max_serviced_duration: std::time::Duration::from_secs(5),
        }
    }

    pub(crate) fn device(&self) -> DrmDeviceKey {
        self.device
    }

    pub(crate) fn contains(&self, key: &AllocationKey) -> bool {
        self.entries.contains_key(key)
    }

    pub(crate) fn has_pending_obligations(&self, key: &AllocationKey) -> bool {
        if let Some(entry) = self.entries.get(key) {
            !entry.availability.borrow().pending_obligations.is_empty()
        } else {
            false
        }
    }

    pub(crate) fn has_pending_obligation(
        &self,
        key: &AllocationKey,
        obligation: ObligationId,
    ) -> bool {
        if let Some(entry) = self.entries.get(key) {
            entry
                .availability
                .borrow()
                .pending_obligations
                .contains_key(&obligation)
        } else {
            false
        }
    }

    pub(crate) fn is_releasable(&self, key: &AllocationKey) -> bool {
        if let Some(entry) = self.entries.get(key) {
            let avail = entry.availability.borrow();
            !avail.frozen && avail.pending_obligations.is_empty()
        } else {
            true
        }
    }

    pub(crate) fn incarnation(&self) -> IncarnationId {
        self.incarnation
    }

    pub(crate) fn set_seat_active(&mut self, active: bool, now: Instant) {
        if self.seat_active && !active {
            // Pausing
            if let Some(last) = self.last_serviced {
                let delta = now.saturating_duration_since(last);
                self.serviced_elapsed = self.serviced_elapsed.saturating_add(delta);
            }
            self.last_serviced = None;
        } else if !self.seat_active && active {
            // Resuming
            self.last_serviced = Some(now);
        }
        self.seat_active = active;
    }

    pub(crate) fn waiters_mut(&mut self) -> &mut WaiterRegistry {
        &mut self.waiters
    }

    pub(crate) fn next_deadline(&self) -> Option<Instant> {
        if self.pending_batches.is_empty() || !self.seat_active {
            return None;
        }
        Some(Instant::now() + std::time::Duration::from_millis(1))
    }

    pub(crate) fn service_completions(
        &mut self,
        now: Instant,
    ) -> Result<Vec<AllocationKey>, ResourceError> {
        if self.seat_active {
            if let Some(last) = self.last_serviced {
                let delta = now.saturating_duration_since(last);
                self.serviced_elapsed = self.serviced_elapsed.saturating_add(delta);
            }
            self.last_serviced = Some(now);
        }

        // Check timeout on pending batches
        if self.serviced_elapsed >= self.max_serviced_duration && !self.pending_batches.is_empty() {
            // Expiry in serviced time: freeze all pending batches, close converted admission
            let expired_batches = std::mem::take(&mut self.pending_batches);
            for batch in expired_batches {
                self.quarantine_gpu_batch(batch, ResourceError::Frozen);
            }
            self.exhausted = true;
            return Err(ResourceError::Frozen);
        }

        let poll_result = self.poll_gpu(now);

        // Service ready allocations and detect eligibility edges
        let available_keys = self.service_ready();

        for key in &available_keys {
            self.waiters.notify_eligible(key.generation);
        }

        poll_result.map(|_| available_keys)
    }

    #[allow(clippy::result_large_err)]
    pub(crate) fn adopt(
        &mut self,
        payload: AllocationPayload,
    ) -> Result<AllocationLease, (ResourceError, AllocationPayload)> {
        if self.exhausted {
            return Err((ResourceError::Exhausted, payload));
        }

        let generation = self.next_generation;
        let use_id = self.next_use_id;

        let next_gen = match generation.checked_add(1) {
            Some(g) => g,
            None => {
                self.exhausted = true;
                return Err((ResourceError::Exhausted, payload));
            }
        };

        let next_use = match use_id.checked_add(1) {
            Some(u) => u,
            None => {
                self.exhausted = true;
                return Err((ResourceError::Exhausted, payload));
            }
        };

        self.next_generation = next_gen;
        self.next_use_id = next_use;

        let key = AllocationKey {
            device: self.device,
            incarnation: self.incarnation,
            generation,
        };

        let entry = Rc::new(AllocationEntry::new(key, payload));
        entry
            .availability
            .borrow_mut()
            .live_uses
            .insert(UseId(use_id), UseKind::Retain);

        self.entries.insert(key, Rc::clone(&entry));

        Ok(AllocationLease::new(
            entry,
            UseId(use_id),
            UseKind::Retain,
            Rc::downgrade(&self.dirty_entries),
        ))
    }

    pub(crate) fn reserve(
        &mut self,
        key: AllocationKey,
        usage: UseKind,
    ) -> Result<AllocationLease, ResourceError> {
        if key.device != self.device || key.incarnation != self.incarnation {
            return Err(ResourceError::WrongIncarnation);
        }
        if self.exhausted {
            return Err(ResourceError::Exhausted);
        }

        let entry = self.entries.get(&key).ok_or(ResourceError::Detached)?;

        let mut avail = entry.availability.borrow_mut();
        if avail.frozen {
            return Err(ResourceError::Frozen);
        }

        if !avail.is_compatible(usage) {
            return Err(ResourceError::Busy);
        }

        let use_id = self.next_use_id;
        let next_use = match use_id.checked_add(1) {
            Some(u) => u,
            None => {
                self.exhausted = true;
                return Err(ResourceError::Exhausted);
            }
        };
        self.next_use_id = next_use;

        avail.live_uses.insert(UseId(use_id), usage);
        drop(avail);

        Ok(AllocationLease::new(
            Rc::clone(entry),
            UseId(use_id),
            usage,
            Rc::downgrade(&self.dirty_entries),
        ))
    }

    pub(crate) fn register(
        &mut self,
        key: AllocationKey,
        kind: ObligationKind,
    ) -> Result<ObligationId, ResourceError> {
        if key.device != self.device || key.incarnation != self.incarnation {
            return Err(ResourceError::WrongIncarnation);
        }
        if self.exhausted {
            return Err(ResourceError::Exhausted);
        }

        let entry = self.entries.get(&key).ok_or(ResourceError::Detached)?;

        let mut avail = entry.availability.borrow_mut();
        if avail.frozen {
            return Err(ResourceError::Frozen);
        }

        let obligation_id = self.next_obligation_id;
        let next_ob = match obligation_id.checked_add(1) {
            Some(o) => o,
            None => {
                self.exhausted = true;
                return Err(ResourceError::Exhausted);
            }
        };
        self.next_obligation_id = next_ob;

        avail
            .pending_obligations
            .insert(ObligationId(obligation_id), kind);

        Ok(ObligationId(obligation_id))
    }

    pub(crate) fn freeze(&mut self, key: AllocationKey) -> Result<(), ResourceError> {
        if key.device != self.device || key.incarnation != self.incarnation {
            return Err(ResourceError::WrongIncarnation);
        }
        let entry = self.entries.get(&key).ok_or(ResourceError::Detached)?;
        entry.availability.borrow_mut().frozen = true;
        self.dirty_entries.borrow_mut().insert(key);
        Ok(())
    }

    pub(crate) fn cancel(
        &mut self,
        key: AllocationKey,
        obligation: ObligationId,
    ) -> Result<(), ResourceError> {
        if key.device != self.device || key.incarnation != self.incarnation {
            return Err(ResourceError::WrongIncarnation);
        }
        let entry = self.entries.get(&key).ok_or(ResourceError::Detached)?;
        let mut avail = entry.availability.borrow_mut();
        if avail.pending_obligations.remove(&obligation).is_none() {
            return Err(ResourceError::InvalidProof);
        }
        drop(avail);
        self.dirty_entries.borrow_mut().insert(key);
        Ok(())
    }

    pub(crate) fn apply_validated_proof(
        &mut self,
        key: AllocationKey,
        obligation: ObligationId,
    ) -> Result<(), ResourceError> {
        if key.device != self.device || key.incarnation != self.incarnation {
            return Err(ResourceError::WrongIncarnation);
        }
        let entry = self.entries.get(&key).ok_or(ResourceError::Detached)?;
        let mut avail = entry.availability.borrow_mut();
        if avail.pending_obligations.remove(&obligation).is_none() {
            return Err(ResourceError::InvalidProof);
        }
        avail.kms_dispositions.remove(&obligation);
        drop(avail);
        self.dirty_entries.borrow_mut().insert(key);
        Ok(())
    }

    pub(crate) fn register_kms(
        &mut self,
        key: AllocationKey,
        commit: crate::kms::owner::identity::CommitId,
        member: GroupMember,
    ) -> Result<ObligationId, ResourceError> {
        let id = self.register(key, ObligationKind::KmsRelease)?;
        let entry = self.entries.get(&key).ok_or(ResourceError::Detached)?;
        entry
            .availability
            .borrow_mut()
            .kms_dispositions
            .insert(id, (member, commit, KmsDisposition::Outstanding));
        Ok(id)
    }

    pub(crate) fn record_kms_discharged(
        &mut self,
        key: AllocationKey,
        obligation: ObligationId,
        commit: crate::kms::owner::identity::CommitId,
        member: GroupMember,
    ) -> Result<(), ResourceError> {
        if key.device != self.device || key.incarnation != self.incarnation {
            return Err(ResourceError::WrongIncarnation);
        }
        let entry = self.entries.get(&key).ok_or(ResourceError::Detached)?;
        let mut avail = entry.availability.borrow_mut();
        if let Some((stored_member, stored_commit, disp)) =
            avail.kms_dispositions.get_mut(&obligation)
        {
            if *stored_commit == commit
                && stored_member.crtc == member.crtc
                && stored_member.topology_generation == member.topology_generation
                && stored_member.crtc_epoch == member.crtc_epoch
            {
                *disp = KmsDisposition::Discharged;
                drop(avail);
                self.dirty_entries.borrow_mut().insert(key);
                Ok(())
            } else {
                Err(ResourceError::InvalidProof)
            }
        } else {
            Err(ResourceError::InvalidProof)
        }
    }

    pub(crate) fn record_device_barrier(&mut self, barrier: DeviceBarrier) {
        if barrier.device() != self.device {
            return;
        }
        for (key, entry) in &self.entries {
            let mut avail = entry.availability.borrow_mut();
            let mut changed = false;
            for (_, _, disp) in avail.kms_dispositions.values_mut() {
                if *disp == KmsDisposition::Outstanding {
                    *disp = KmsDisposition::Superseded(barrier);
                    changed = true;
                }
            }
            if changed {
                self.dirty_entries.borrow_mut().insert(*key);
            }
        }
    }

    pub(crate) fn apply_teardown_release(
        &mut self,
        proof: TeardownRelease,
    ) -> Result<(), ResourceError> {
        if proof.incarnation != self.incarnation {
            return Err(ResourceError::WrongIncarnation);
        }
        // 1. Validation phase
        for key in &proof.entries {
            if key.device != self.device || key.incarnation != self.incarnation {
                return Err(ResourceError::WrongIncarnation);
            }
            let entry = self.entries.get(key).ok_or(ResourceError::Detached)?;
            let avail = entry.availability.borrow();
            if !avail.frozen {
                return Err(ResourceError::InvalidState);
            }
            for (ob_id, ob_kind) in &avail.pending_obligations {
                if *ob_kind == ObligationKind::KmsRelease {
                    let disp = avail
                        .kms_dispositions
                        .get(ob_id)
                        .map(|(_, _, d)| *d)
                        .unwrap_or(KmsDisposition::Outstanding);
                    match disp {
                        KmsDisposition::Discharged => {}
                        KmsDisposition::Superseded(barrier) if barrier.device() == self.device => {}
                        _ => {
                            // Outstanding KMS obligation or mismatched device barrier
                            return Err(ResourceError::InvalidProof);
                        }
                    }
                } else {
                    // Non-KMS obligations must have independent proof
                    return Err(ResourceError::InvalidProof);
                }
            }
        }

        // 2. Teardown release execution
        for key in &proof.entries {
            if let Some(entry) = self.entries.get(key) {
                let mut avail = entry.availability.borrow_mut();
                avail.frozen = false;
                avail
                    .pending_obligations
                    .retain(|_, kind| *kind != ObligationKind::KmsRelease);
                avail.kms_dispositions.clear();
                drop(avail);
                self.dirty_entries.borrow_mut().insert(*key);
            }
        }
        Ok(())
    }

    pub(crate) fn service_ready(&mut self) -> Vec<AllocationKey> {
        let dirty_keys: BTreeSet<AllocationKey> =
            std::mem::take(&mut *self.dirty_entries.borrow_mut());
        let mut transitions = Vec::new();
        for key in dirty_keys {
            if let Some(entry) = self.entries.get(&key) {
                if can_destroy(entry) {
                    if let Some(removed) = self.entries.remove(&key) {
                        removed.take_payload();
                    }
                    transitions.push(key);
                } else {
                    transitions.push(key);
                }
            }
        }
        transitions
    }

    pub(crate) fn retain_storage(
        &mut self,
        source: &StorageLease,
    ) -> Result<StorageLease, ResourceError> {
        let key = source.allocation.key();
        let new_alloc_lease = self.reserve(key, UseKind::Retain)?;
        Ok(StorageLease {
            allocation: new_alloc_lease,
            pixels: source.pixels.clone(),
        })
    }

    pub(crate) fn with_storage_read<T>(
        &mut self,
        lease: &StorageLease,
        f: impl FnOnce(&StorageAllocation) -> T,
    ) -> Result<T, ResourceError> {
        let key = lease.allocation.key();
        let read_lease = self.reserve(key, UseKind::Read)?;
        let entry = self.entries.get(&key).ok_or(ResourceError::Detached)?;
        let payload = entry.payload.borrow();
        let alloc = match payload.as_ref() {
            Some(AllocationPayload::Storage(alloc)) => alloc,
            _ => return Err(ResourceError::Detached),
        };
        let res = f(alloc);
        drop(payload);
        drop(read_lease);
        Ok(res)
    }

    pub(crate) fn with_storage_write<T>(
        &mut self,
        lease: &StorageLease,
        f: impl FnOnce(&mut StorageAllocation) -> T,
    ) -> Result<T, ResourceError> {
        let key = lease.allocation.key();
        let write_lease = self.reserve(key, UseKind::Write)?;
        let entry = self.entries.get(&key).ok_or(ResourceError::Detached)?;
        let mut payload = entry.payload.borrow_mut();
        let alloc = match payload.as_mut() {
            Some(AllocationPayload::Storage(alloc)) => alloc,
            _ => return Err(ResourceError::Detached),
        };
        let res = f(alloc);
        drop(payload);
        drop(write_lease);
        Ok(res)
    }

    pub(crate) fn register_batch(&mut self, batch: CoreRetirementBatch) {
        self.pending_batches.push(batch);
    }

    pub(crate) fn pending_batches(&self) -> &[CoreRetirementBatch] {
        &self.pending_batches
    }

    pub(crate) fn quarantined_batches(&self) -> &[(CoreRetirementBatch, ResourceError)] {
        &self.quarantined_batches
    }

    pub(crate) fn poll_gpu(&mut self, _now: Instant) -> Result<(), ResourceError> {
        let batches = std::mem::take(&mut self.pending_batches);
        let mut pending = Vec::new();
        let mut failed = false;

        for batch in batches {
            match batch.ticket_status() {
                Ok(false) => pending.push(batch),
                Ok(true) => match self.validate_gpu_batch(batch) {
                    Ok(prepared) => self.commit_gpu_batch(prepared),
                    Err((error, retained)) => {
                        self.quarantine_gpu_batch(retained, error);
                        failed = true;
                    }
                },
                Err(_) => {
                    self.quarantine_gpu_batch(batch, ResourceError::Frozen);
                    failed = true;
                }
            }
        }

        self.pending_batches = pending;
        if failed {
            Err(ResourceError::Frozen)
        } else {
            Ok(())
        }
    }

    #[allow(clippy::result_large_err)]
    pub(crate) fn validate_gpu_batch(
        &self,
        batch: CoreRetirementBatch,
    ) -> Result<ValidatedGpuBatch, (ResourceError, CoreRetirementBatch)> {
        let mut confirmed_entries = Vec::new();

        if let Some(ob) = &batch.obligation {
            for &(key, obligation_id) in ob.entries() {
                if key.device != self.device || key.incarnation != self.incarnation {
                    return Err((ResourceError::WrongIncarnation, batch));
                }
                let Some(entry) = self.entries.get(&key) else {
                    return Err((ResourceError::Detached, batch));
                };
                let avail = entry.availability.borrow();
                if avail.frozen {
                    return Err((ResourceError::Frozen, batch));
                }
                if !avail.pending_obligations.contains_key(&obligation_id) {
                    return Err((ResourceError::InvalidProof, batch));
                }
                confirmed_entries.push((key, obligation_id));
            }
        }

        if let Some(read_ob) = &batch.read_obligation {
            let key = read_ob.source_key();
            let obligation_id = read_ob.source_obligation();
            if key.device != self.device || key.incarnation != self.incarnation {
                return Err((ResourceError::WrongIncarnation, batch));
            }
            let Some(entry) = self.entries.get(&key) else {
                return Err((ResourceError::Detached, batch));
            };
            let avail = entry.availability.borrow();
            if avail.frozen {
                return Err((ResourceError::Frozen, batch));
            }
            if !avail.pending_obligations.contains_key(&obligation_id) {
                return Err((ResourceError::InvalidProof, batch));
            }
            confirmed_entries.push((key, obligation_id));

            if let Some(staging_lease) = &read_ob.staging_lease
                && let Some(staging_ob) = read_ob.staging_obligation
            {
                let s_key = staging_lease.key();
                if s_key.device != self.device || s_key.incarnation != self.incarnation {
                    return Err((ResourceError::WrongIncarnation, batch));
                }
                let Some(s_entry) = self.entries.get(&s_key) else {
                    return Err((ResourceError::Detached, batch));
                };
                let s_avail = s_entry.availability.borrow();
                if s_avail.frozen {
                    return Err((ResourceError::Frozen, batch));
                }
                if !s_avail.pending_obligations.contains_key(&staging_ob) {
                    return Err((ResourceError::InvalidProof, batch));
                }
                confirmed_entries.push((s_key, staging_ob));
            }
        }

        Ok(ValidatedGpuBatch {
            batch,
            confirmed_entries,
        })
    }

    pub(crate) fn commit_gpu_batch(&mut self, prepared: ValidatedGpuBatch) {
        let ValidatedGpuBatch {
            batch,
            confirmed_entries,
        } = prepared;

        for (key, obligation_id) in confirmed_entries {
            if let Some(entry) = self.entries.get(&key) {
                entry
                    .availability
                    .borrow_mut()
                    .pending_obligations
                    .remove(&obligation_id);
                self.dirty_entries.borrow_mut().insert(key);
            }
        }

        drop(batch);
    }

    pub(crate) fn quarantine_gpu_batch(
        &mut self,
        batch: CoreRetirementBatch,
        reason: ResourceError,
    ) {
        let mut keys_to_freeze = Vec::new();
        if let Some(ob) = &batch.obligation {
            for &(key, _) in ob.entries() {
                keys_to_freeze.push(key);
            }
        }
        if let Some(read_ob) = &batch.read_obligation {
            keys_to_freeze.push(read_ob.source_key());
            if let Some(staging_lease) = &read_ob.staging_lease {
                keys_to_freeze.push(staging_lease.key());
            }
        }

        self.quarantined_batches.push((batch, reason));

        for key in keys_to_freeze {
            if let Some(entry) = self.entries.get(&key) {
                entry.availability.borrow_mut().frozen = true;
                self.dirty_entries.borrow_mut().insert(key);
            }
        }
    }
}
