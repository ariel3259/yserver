pub(crate) mod availability;
pub(crate) mod drm_cleanup;
pub(crate) mod gpu;
pub(crate) mod lease;
pub(crate) mod scanout;
pub(crate) mod storage;

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
pub(crate) use drm_cleanup::{
    CleanupIo, DeviceCleanupIo, DirectFramebufferAllocation, DrmCleanupRegistry, DrmCleanupRight,
    FakeFamilyInventory, FileFamilyClosed, GemOwner, RightState,
};
#[allow(unused_imports)]
pub(crate) use gpu::{CoreRetirementBatch, GpuObligation, ReadObligation, ValidatedGpuBatch};
pub(crate) use lease::AllocationLease;
#[allow(unused_imports)]
pub(crate) use scanout::{
    CopiedSourceAllocation, FileOwnedBacking, ManagedScanoutToken, ScanoutAllocation, SharedBacking,
};
#[allow(unused_imports)]
pub(crate) use storage::{
    PixelIdentity, StorageAccessError, StorageAllocation, StorageBacking, StorageLease,
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
        }
    }

    pub(crate) fn device(&self) -> DrmDeviceKey {
        self.device
    }

    pub(crate) fn contains(&self, key: &AllocationKey) -> bool {
        self.entries.contains_key(key)
    }

    pub(crate) fn incarnation(&self) -> IncarnationId {
        self.incarnation
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
        drop(avail);
        self.dirty_entries.borrow_mut().insert(key);
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
