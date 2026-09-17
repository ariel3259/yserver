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
mod adapter_tests;
#[cfg(test)]
mod guard_tests;
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
pub(crate) use commit::{
    CommitResourceConsumer, CommitResources, GroupMember, PresentRelease, cancel_pre_ipc_commit,
    register_commit_dependencies,
};
#[allow(unused_imports)]
pub(crate) use completion::{ResourceConsumer, ResourceWaiter, WaiterRegistry};
#[allow(unused_imports)]
pub(crate) use drm_cleanup::{
    CleanupIo, DeviceCleanupIo, DirectFramebufferAllocation, DrmCleanupRegistry, DrmCleanupRight,
    FamilyInventory, FileFamilyClosed, GemOwner, PoolHuskRegistration, RightState,
};
#[allow(unused_imports)]
use gpu::ValidatedGpuBatch;
#[allow(unused_imports)]
pub(crate) use gpu::{CoreRetirementBatch, GpuObligation, ReadObligation};
#[allow(unused_imports)]
pub(crate) use handoff::{
    CompletionIngress, DeviceBarrier, HandoffRouter, IncarnationBundle, KmsDisposition,
    RecipientSlot, TeardownRelease,
};
// B-6: `RetainingSupervisor` is a test fixture (handoff.rs) -- only visible
// under `#[cfg(test)]`, same as everything that constructs one.
#[cfg(test)]
#[allow(unused_imports)]
pub(crate) use handoff::RetainingSupervisor;
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
#[cfg(test)]
#[allow(unused_imports)]
pub(crate) use transport::FakeDirectOwnershipState;
#[allow(unused_imports)]
pub(crate) use transport::{
    DirectOwnershipState, HandoverPermit, OwnerWriteGrant, RecipientReservation, TransportGate,
    TransportGateHandle, TransportState, WriterClass, WriterCoverageProof,
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

impl AllocationPayload {
    /// True when this payload still holds a counted alias of the DRM open
    /// file description that must go through the registry -- a real
    /// barrier discharge or the normal per-payload path -- rather than an
    /// ordinary Rust `Drop` (F2-M1): `Scanout` with `file_owned: Some`, or
    /// `DirectFramebuffer` with its right/device not yet discharged. Every
    /// payload kind that can carry this alias must be listed here; `adopt`,
    /// `service_ready`/`service_ready_with_registry` and
    /// `apply_teardown_release` all key off this one method so a new kind
    /// cannot silently reopen the hole "additive" left for
    /// `DirectFramebuffer` the first time (F2-M1).
    pub(crate) fn file_owned_alias_present(&self) -> bool {
        match self {
            AllocationPayload::Scanout(alloc) => alloc.file_owned().is_some(),
            AllocationPayload::DirectFramebuffer(alloc) => {
                alloc.right().is_some() || alloc.device.is_some()
            }
            #[cfg(test)]
            AllocationPayload::Spy(_) => false,
            AllocationPayload::Storage(_) | AllocationPayload::CopiedSource(_) => false,
            AllocationPayload::Unused(never) => match *never {},
        }
    }

    /// Discharges the file-owned half through `registry`, for whichever
    /// payload kind carries one; a no-op `Ok(())` for the rest. The single
    /// dispatch point `service_ready_with_registry` calls, so a new
    /// file-owned-carrying kind only needs its arm added here.
    pub(crate) fn discharge_file_owned(
        &mut self,
        registry: &mut DrmCleanupRegistry,
    ) -> Result<(), std::io::Error> {
        match self {
            AllocationPayload::Scanout(alloc) => alloc.discharge_file_owned(registry),
            AllocationPayload::DirectFramebuffer(alloc) => alloc.discharge_file_owned(registry),
            #[cfg(test)]
            AllocationPayload::Spy(_) => Ok(()),
            AllocationPayload::Storage(_) | AllocationPayload::CopiedSource(_) => Ok(()),
            AllocationPayload::Unused(never) => match *never {},
        }
    }
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
    /// Spec 4.2 (stage 2c-i debt): the transport an uncertain or unwindable
    /// GPU submission must close (2c-i design section 4). `None` in
    /// production, where no gate is installed (R8).
    transport_gate: Option<TransportGateHandle>,
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
            transport_gate: None,
        }
    }

    /// Round-1 B-2: the gate this service closes must be this service's own
    /// transport. A handle for another device or incarnation is refused, and
    /// so is a second, different gate -- replacing one would leave the
    /// transport the service's outstanding work belongs to open. Re-installing
    /// the same gate is idempotent.
    pub(crate) fn set_transport_gate(
        &mut self,
        gate: TransportGateHandle,
    ) -> Result<(), ResourceError> {
        if gate.device() != self.device || gate.incarnation() != self.incarnation {
            return Err(ResourceError::WrongIncarnation);
        }
        if let Some(installed) = &self.transport_gate
            && !installed.same_gate(&gate)
        {
            return Err(ResourceError::InvalidState);
        }
        self.transport_gate = Some(gate);
        Ok(())
    }

    /// Closes the installed transport gate, if any.
    pub(crate) fn close_transport_gate(&self) {
        if let Some(gate) = &self.transport_gate {
            gate.close_gate();
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

    /// M-16: progress (polling an already-signalled ticket) is never
    /// suppressed by seat inactivity -- only the serviced-time *budget*
    /// pauses (`service_completions` below gates its `serviced_elapsed`
    /// advance on `seat_active`). A pending batch's deadline is therefore
    /// returned regardless of `seat_active`: without it, `next_wakeup`
    /// (which chains this unconditionally) would return `None` while
    /// VT-away/DPMS-off with nothing else pending, and the core loop could
    /// block in `poll()` with no timeout -- so a ticket that signals during
    /// that window is never observed until an unrelated fd wakes the loop.
    /// The pre-fix behaviour (`None` while inactive) is exactly the defect:
    /// it conflated "don't count this time toward the expiry budget" with
    /// "don't bother looking again," which are different things (R9: the
    /// pending deadline counts serviced time and pauses while the seat is
    /// inactive -- servicing itself does not).
    pub(crate) fn next_deadline(&self) -> Option<Instant> {
        if self.pending_batches.is_empty() {
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

        // B-11: each pending batch carries its own serviced-time deadline,
        // computed from its own registration (`register_batch`). Expiry
        // quarantines only that batch -- never a service-wide `exhausted`
        // flip, and never by comparing a single global accumulator against
        // one shared threshold, which would expire every batch in the
        // service at once regardless of when each was actually registered.
        let mut still_pending = Vec::with_capacity(self.pending_batches.len());
        let mut expired = Vec::new();
        for batch in std::mem::take(&mut self.pending_batches) {
            match batch.serviced_deadline {
                Some(deadline) if self.serviced_elapsed >= deadline => expired.push(batch),
                _ => still_pending.push(batch),
            }
        }
        self.pending_batches = still_pending;
        let any_expired = !expired.is_empty();
        for batch in expired {
            // Expiry is never a completion proof (R9): the batch stays
            // rooted for teardown, exactly like a failed ticket.
            self.quarantine_gpu_batch(batch, ResourceError::Frozen);
        }

        let poll_result = self.poll_gpu(now);

        // Service ready allocations and detect eligibility edges
        let available_keys = self.service_ready();

        for key in &available_keys {
            self.waiters.notify_eligible(key.generation);
        }

        if any_expired {
            return Err(ResourceError::Frozen);
        }

        poll_result.map(|_| available_keys)
    }

    /// Refuses (F2-M1) any payload whose file-owned half is still live --
    /// `Scanout` with `file_owned: Some`, or `DirectFramebuffer` with its
    /// right/device not yet discharged -- since a plain `adopt` never
    /// registers the alias with a registry, and this crate has no `Drop`
    /// that would otherwise close it (M-23's whole point). Such a payload
    /// must go through `adopt_with_registry`.
    #[allow(clippy::result_large_err)]
    pub(crate) fn adopt(
        &mut self,
        payload: AllocationPayload,
    ) -> Result<AllocationLease, (ResourceError, AllocationPayload)> {
        if payload.file_owned_alias_present() {
            return Err((ResourceError::InvalidState, payload));
        }
        self.adopt_unchecked(payload)
    }

    #[allow(clippy::result_large_err)]
    fn adopt_unchecked(
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

    /// The only way to adopt a payload whose file-owned half is live (`adopt`
    /// refuses those, F2-M1): registers the alias with `registry` so the
    /// fd-family barrier can discharge it instead of waiting on it (R5,
    /// B-2). Covers every payload kind `file_owned_alias_present` does, not
    /// just `Scanout` (F2-M1) -- `DirectFramebuffer` carries the same kind
    /// of alias and had the same hole open beside the original fix.
    #[allow(clippy::result_large_err)]
    pub(crate) fn adopt_with_registry(
        &mut self,
        payload: AllocationPayload,
        registry: &mut DrmCleanupRegistry,
    ) -> Result<AllocationLease, (ResourceError, AllocationPayload)> {
        let carries_file_owned_alias = payload.file_owned_alias_present();
        let lease = self.adopt_unchecked(payload)?;
        if carries_file_owned_alias {
            registry.register_payload_alias(lease.key());
        }
        Ok(lease)
    }

    /// Reclaims a payload adopted moments ago, for an all-or-nothing
    /// multi-payload adoption that must unwind when a later step fails
    /// (F2-M2). Only succeeds if the lease is still the entry's sole use
    /// and nothing has registered an obligation on it since; otherwise
    /// something else is already relying on it, it is too late to safely
    /// hard-reclaim, and the lease is handed back so the caller's ordinary
    /// `drop` releases it through the normal lifecycle instead.
    pub(crate) fn release_fresh_adoption(
        &mut self,
        lease: AllocationLease,
    ) -> Result<AllocationPayload, AllocationLease> {
        let key = lease.key();
        let reclaimable = self.entries.get(&key).is_some_and(|entry| {
            entry.live_use_count() == 1 && entry.pending_obligation_count() == 0
        });
        if !reclaimable {
            return Err(lease);
        }
        drop(lease);
        Ok(self
            .entries
            .remove(&key)
            .and_then(|entry| entry.take_payload())
            .expect("just-adopted entry with its sole retain use just removed has a payload"))
    }

    /// Whether `adopt`/`adopt_with_registry` would refuse for exhaustion
    /// right now (F2-M2): lets a multi-step caller check admission before
    /// extracting physical resources it would otherwise have nowhere to
    /// put back cheaply.
    pub(crate) fn is_exhausted(&self) -> bool {
        self.exhausted
    }

    /// F2b-m1: real exhaustion only happens after `u64::MAX` generations/
    /// uses/obligations or a serviced-time expiry, neither reachable in a
    /// unit test. This forces the same flag directly so
    /// `register_managed_scanout_bo`'s pre-extraction `is_exhausted()`
    /// guard (F2-M2) can be exercised deterministically.
    #[cfg(test)]
    pub(crate) fn force_exhausted_for_tests(&mut self) {
        self.exhausted = true;
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

    pub(crate) fn is_frozen(&self, key: &AllocationKey) -> bool {
        self.entries.get(key).map(|e| e.frozen()).unwrap_or(false)
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
        // F-14/S2-m1: mark a cancelled KMS registration `Cancelled` rather
        // than leaving it `Outstanding` (stale) or removing it outright
        // (which would make `cancel` indistinguishable from
        // `apply_validated_proof`, below). `record_device_barrier` only
        // ever flips an `Outstanding` disposition, so this also stops the
        // stale-entry bug: a cancelled registration can no longer be
        // mistaken for a live one and flipped to `Superseded` (with a
        // spurious dirty mark) for a commit that never happened.
        if let Some((_, _, disp)) = avail.kms_dispositions.get_mut(&obligation) {
            *disp = KmsDisposition::Cancelled;
        }
        drop(avail);
        self.dirty_entries.borrow_mut().insert(key);
        Ok(())
    }

    pub(in crate::kms::render::resources) fn validate_proof_target(
        &self,
        key: AllocationKey,
        obligation: ObligationId,
    ) -> Result<(), ResourceError> {
        if key.device != self.device || key.incarnation != self.incarnation {
            return Err(ResourceError::WrongIncarnation);
        }
        let entry = self.entries.get(&key).ok_or(ResourceError::Detached)?;
        let avail = entry.availability.borrow();
        if !avail.pending_obligations.contains_key(&obligation) {
            return Err(ResourceError::InvalidProof);
        }
        Ok(())
    }

    /// Private to `resources`: producers must correlate real evidence before
    /// calling this (vocabulary, Task 1); it is not a public `complete` API.
    pub(in crate::kms::render::resources) fn apply_validated_proof(
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

    /// Shim for `store.rs`'s tests, which live outside `resources` and drive
    /// proof application directly to control ordering. Production code must
    /// go through a producer adapter, never this.
    #[cfg(test)]
    pub(crate) fn apply_validated_proof_for_tests(
        &mut self,
        key: AllocationKey,
        obligation: ObligationId,
    ) -> Result<(), ResourceError> {
        self.apply_validated_proof(key, obligation)
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

    pub(crate) fn kms_disposition(
        &self,
        key: AllocationKey,
        obligation: ObligationId,
    ) -> Option<handoff::KmsDisposition> {
        let entry = self.entries.get(&key)?;
        entry
            .availability
            .borrow()
            .kms_dispositions
            .get(&obligation)
            .map(|(_, _, disp)| *disp)
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
            // M-10: teardown release proves only the KMS obligation's own
            // disposition. A live file-owned right/gbm_bo/device alias
            // underneath it is a separate proof (the fd-family barrier's own
            // discharge, or ordinary release) that this proof does not
            // stand in for -- letting it through here is the mechanism
            // behind B-2's ioctl-after-barrier. Checked via the same
            // dispatcher `adopt`/`service_ready_with_registry` use (F2-M1),
            // so it covers `DirectFramebuffer` too, not only `Scanout`.
            if entry
                .payload
                .borrow()
                .as_ref()
                .is_some_and(AllocationPayload::file_owned_alias_present)
            {
                return Err(ResourceError::InvalidProof);
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
                    // F2-M1: this path has no registry to discharge a live
                    // file-owned half through, so destroying the entry here
                    // would be exactly the undischarged drop B-2 closed for
                    // `service_ready_with_registry`. Re-dirty instead and
                    // wait for that call.
                    if entry
                        .payload
                        .borrow()
                        .as_ref()
                        .is_some_and(AllocationPayload::file_owned_alias_present)
                    {
                        self.dirty_entries.borrow_mut().insert(key);
                        transitions.push(key);
                        continue;
                    }
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

    /// Like `service_ready`, but discharges a payload's file-owned half
    /// through `registry` before dropping it (B-2): neither
    /// `ScanoutAllocation`/`FileOwnedBacking` nor `DirectFramebufferAllocation`
    /// has a `Drop` that closes the DRM framebuffer/GEM handle, so an
    /// ordinary `service_ready` drop of a still-live file-owned half is a
    /// silent leak, not a release. This is the normal-path counterpart to
    /// the Task-9 barrier discharge: KMS proof arrived, so the entry is
    /// destroyable, and the right closes here rather than through the
    /// barrier. Dispatches through `AllocationPayload::discharge_file_owned`
    /// (F2-M1), so it covers every kind `file_owned_alias_present` does.
    pub(crate) fn service_ready_with_registry(
        &mut self,
        registry: &mut DrmCleanupRegistry,
    ) -> Vec<AllocationKey> {
        let dirty_keys: BTreeSet<AllocationKey> =
            std::mem::take(&mut *self.dirty_entries.borrow_mut());
        let mut transitions = Vec::new();
        for key in dirty_keys {
            if let Some(entry) = self.entries.get(&key) {
                if can_destroy(entry) {
                    let discharge_result = {
                        let mut payload = entry.payload.borrow_mut();
                        match payload.as_mut() {
                            Some(p) => p.discharge_file_owned(registry),
                            None => Ok(()),
                        }
                    };
                    match discharge_result {
                        Ok(()) => {
                            // F2-B2: unregister before removing the entry --
                            // a stale key here makes the barrier walk hand
                            // it to a callback with nothing left to look up.
                            registry.unregister_payload_alias(key);
                            if let Some(removed) = self.entries.remove(&key) {
                                removed.take_payload();
                            }
                            transitions.push(key);
                        }
                        Err(_) => {
                            // F6: never a bare discard. The right kept its
                            // `FramebufferRemoved` retry state inside the
                            // still-rooted payload (R3); re-mark dirty so
                            // the next service tick retries the discharge
                            // instead of the entry being silently dropped
                            // undischarged.
                            self.dirty_entries.borrow_mut().insert(key);
                        }
                    }
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

    pub(crate) fn share_storage_read(
        &mut self,
        source: &StorageLease,
    ) -> Result<StorageLease, ResourceError> {
        let key = source.allocation.key();
        let new_alloc_lease = self.reserve(key, UseKind::Read)?;
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

    /// F4b-B1/F4-M3: the scanout counterpart of `with_storage_read`. Once a
    /// bo is converted to managed ownership (`register_managed_scanout_bo`,
    /// F-2), its real image/staging live in the `ScanoutAllocation`
    /// payload -- the pool's `ScanoutBo` is left an emptied husk
    /// (`take_physical_backing`). A caller that needs those fields must
    /// reserve a `Read` use on the bo's managed key and take them from the
    /// payload under that lease, never from the (possibly husked) pool
    /// struct directly.
    pub(crate) fn with_scanout_read<T>(
        &mut self,
        lease: &AllocationLease,
        f: impl FnOnce(&ScanoutAllocation) -> T,
    ) -> Result<T, ResourceError> {
        let key = lease.key();
        let read_lease = if lease.kind == UseKind::Read {
            None
        } else {
            Some(self.reserve(key, UseKind::Read)?)
        };
        let entry = self.entries.get(&key).ok_or(ResourceError::Detached)?;
        let payload = entry.payload.borrow();
        let alloc = match payload.as_ref() {
            Some(AllocationPayload::Scanout(alloc)) => alloc,
            _ => return Err(ResourceError::Detached),
        };
        let res = f(alloc);
        drop(payload);
        drop(read_lease);
        Ok(res)
    }

    /// F4b-B1/F4-M3: the scanout counterpart of `with_storage_write`. See
    /// `with_scanout_read` -- a managed scene-submission write target must
    /// go through the payload the same way.
    pub(crate) fn with_scanout_write<T>(
        &mut self,
        lease: &AllocationLease,
        f: impl FnOnce(&mut ScanoutAllocation) -> T,
    ) -> Result<T, ResourceError> {
        let key = lease.key();
        let write_lease = if lease.kind == UseKind::Write {
            None
        } else {
            Some(self.reserve(key, UseKind::Write)?)
        };
        let entry = self.entries.get(&key).ok_or(ResourceError::Detached)?;
        let mut payload = entry.payload.borrow_mut();
        let alloc = match payload.as_mut() {
            Some(AllocationPayload::Scanout(alloc)) => alloc,
            _ => return Err(ResourceError::Detached),
        };
        let res = f(alloc);
        drop(payload);
        drop(write_lease);
        Ok(res)
    }

    /// B-11: stamps `batch` with its own serviced-time deadline --
    /// `serviced_elapsed` (this service's cumulative *serviced* time, as of
    /// right now) plus `max_serviced_duration`, both with checked
    /// arithmetic. That deadline is fixed at registration and never
    /// re-derived from a shared/global counter later, so this batch's
    /// expiry depends only on how much serviced time passes *after* it
    /// registers -- never on how much service time other, earlier batches
    /// already consumed. A checked-add overflow (an unrepresentable
    /// deadline) fails closed: the batch is quarantined immediately rather
    /// than admitted to `pending_batches` to be polled forever.
    pub(crate) fn register_batch(&mut self, mut batch: CoreRetirementBatch) {
        match self
            .serviced_elapsed
            .checked_add(self.max_serviced_duration)
        {
            Some(deadline) => {
                batch.serviced_deadline = Some(deadline);
                self.pending_batches.push(batch);
            }
            None => self.quarantine_gpu_batch(batch, ResourceError::Frozen),
        }
    }

    pub(crate) fn pending_batches(&self) -> &[CoreRetirementBatch] {
        &self.pending_batches
    }

    /// Test-only (F4): lets a mechanism test flip a registered batch's
    /// `test_ticket_status` in place, without needing to fabricate a second,
    /// independently-signaled ticket for every step of a multi-poll
    /// scenario. `GpuObligation.context` is `Option<Arc<VkContext>>`
    /// (F4-B1); this only controls what `ticket_status()`'s `#[cfg(test)]`
    /// override reports, exactly as `CoreRetirementBatch::test_ticket_status`
    /// already does before registration.
    #[cfg(test)]
    pub(crate) fn pending_batches_mut(&mut self) -> &mut [CoreRetirementBatch] {
        &mut self.pending_batches
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

    // `pub(in ...)`, not `pub(crate)` (M-23/F7): `ValidatedGpuBatch` itself
    // is private to `resources` (`gpu.rs`'s `pub(super)`), and nothing
    // outside this module ever calls this directly -- only `poll_gpu`
    // does. A `pub(crate)` signature returning a type callers outside
    // `resources` cannot even name is exactly the private-interface
    // mismatch rustc's `private_interfaces` lint (`-D warnings`) catches.
    #[allow(clippy::result_large_err)]
    pub(in crate::kms::render::resources) fn validate_gpu_batch(
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

    pub(in crate::kms::render::resources) fn commit_gpu_batch(
        &mut self,
        prepared: ValidatedGpuBatch,
    ) {
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
