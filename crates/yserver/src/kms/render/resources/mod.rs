pub(crate) mod availability;
pub(crate) mod drm_cleanup;
pub(crate) mod lease;
pub(crate) mod storage;

#[cfg(test)]
pub(crate) mod tests;

use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
    rc::Rc,
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
pub(crate) use lease::AllocationLease;
#[allow(unused_imports)]
pub(crate) use storage::{
    PixelIdentity, StorageAccessError, StorageAllocation, StorageBacking, StorageLease,
};

#[allow(dead_code)]
#[derive(Debug)]
pub(crate) enum AllocationPayload {
    #[cfg(test)]
    Spy(tests::SpyAllocation),
    DirectFramebuffer(DirectFramebufferAllocation),
    Storage(storage::StorageAllocation),
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
}
