use std::{cell::Cell, collections::BTreeSet, io, num::NonZeroU32, rc::Rc};

use drm::{
    buffer::Handle as DrmBufferHandle,
    control::{Device as ControlDevice, framebuffer},
};

use super::{
    AllocationKey, AllocationPayload, ResourceError,
    capacity::{DirectRole, RoleReservation},
    lease::AllocationLease,
};
use crate::{drm::Device, kms::owner::identity::IncarnationId, platform::drm::DrmDeviceKey};

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GemOwner {
    Right,
    Gbm,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RightState {
    Registered,
    FramebufferRemoved,
    Discharged,
    Frozen,
    Closed,
}

#[allow(dead_code)]
#[derive(Debug)]
pub(crate) struct DrmCleanupRight {
    device_key: DrmDeviceKey,
    incarnation: IncarnationId,
    fb: u32,
    gem: u32,
    gem_owner: GemOwner,
    state: RightState,
}

#[allow(dead_code)]
impl DrmCleanupRight {
    /// Private to `resources`: a right is only ever minted by
    /// `DrmCleanupRegistry::register_right`, never assembled by a caller.
    pub(in crate::kms::render::resources) fn new(
        device_key: DrmDeviceKey,
        incarnation: IncarnationId,
        fb: u32,
        gem: u32,
        gem_owner: GemOwner,
    ) -> Self {
        Self {
            device_key,
            incarnation,
            fb,
            gem,
            gem_owner,
            state: RightState::Registered,
        }
    }

    pub(crate) fn fb(&self) -> u32 {
        self.fb
    }

    pub(crate) fn gem(&self) -> u32 {
        self.gem
    }

    pub(crate) fn gem_owner(&self) -> GemOwner {
        self.gem_owner
    }

    pub(crate) fn state(&self) -> RightState {
        self.state
    }

    pub(crate) fn mark_closed(&mut self) {
        self.state = RightState::Closed;
    }
}

/// The role charge already held when a cleanup right became uncertain. The
/// pending owner moves this value; it never reserves a replacement role.
#[allow(dead_code)]
#[derive(Debug)]
pub(crate) enum CleanupCharge {
    Preparing(RoleReservation),
    FinalRole(RoleReservation),
}

impl CleanupCharge {
    pub(crate) fn role(&self) -> DirectRole {
        match self {
            Self::Preparing(slot) | Self::FinalRole(slot) => slot.role(),
        }
    }

    pub(crate) fn release(self) {
        match self {
            Self::Preparing(slot) | Self::FinalRole(slot) => slot.release_after_cleanup(),
        }
    }
}

#[derive(Debug)]
struct PendingCleanupEntry {
    payload: AllocationPayload,
    charge: CleanupCharge,
}

#[allow(dead_code)]
#[derive(Debug)]
pub(crate) struct FileFamilyClosed {
    pub(crate) device_key: DrmDeviceKey,
    pub(crate) incarnation: IncarnationId,
    _private: (),
}

/// Spec 4.1 (stage 2c-i debt): one pool husk's `Rc<drm::Device>` alias,
/// held by the registry entry that counts it. Minted only by
/// `DrmCleanupRegistry::register_pool_husk`, which takes the alias by value;
/// consumed by value by `unregister_pool_husk`, which validates it against
/// the registry's own identity and drops the alias as it uncounts it -- so
/// the inventory can never reach zero while the alias is still alive
/// (round-1 B-1). Dropping the registration undischarged fails closed: its
/// registry can never mint `FileFamilyClosed` again (the lost-role-token
/// rule).
pub(crate) struct PoolHuskRegistration {
    device_key: DrmDeviceKey,
    incarnation: IncarnationId,
    accounting: Rc<Cell<bool>>,
    alias: Option<Rc<crate::drm::Device>>,
    discharged: bool,
}

impl std::fmt::Debug for PoolHuskRegistration {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PoolHuskRegistration")
            .field("device_key", &self.device_key)
            .field("incarnation", &self.incarnation)
            .field("alias_held", &self.alias.is_some())
            .field("discharged", &self.discharged)
            .finish()
    }
}

impl Drop for PoolHuskRegistration {
    fn drop(&mut self) {
        if !self.discharged {
            self.accounting.set(true);
        }
    }
}

#[allow(dead_code)]
pub(crate) trait CleanupIo {
    fn remove_fb(&mut self, fb: u32) -> io::Result<()>;
    fn close_gem(&mut self, gem: u32) -> io::Result<()>;
}

#[allow(dead_code)]
pub(crate) struct DeviceCleanupIo {
    device: Rc<Device>,
}

#[allow(dead_code)]
impl DeviceCleanupIo {
    pub(crate) fn new(device: Rc<Device>) -> Self {
        Self { device }
    }
}

impl CleanupIo for DeviceCleanupIo {
    fn remove_fb(&mut self, fb: u32) -> io::Result<()> {
        let nz = NonZeroU32::new(fb).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid zero framebuffer handle",
            )
        })?;
        let handle = framebuffer::Handle::from(nz);
        self.device.destroy_framebuffer(handle)?;
        Ok(())
    }

    fn close_gem(&mut self, gem: u32) -> io::Result<()> {
        let nz = NonZeroU32::new(gem).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid zero gem buffer handle",
            )
        })?;
        let handle = DrmBufferHandle::from(nz);
        self.device.close_buffer(handle)?;
        Ok(())
    }
}

/// The real R5 preconditions for the fd-family barrier: every submitter and
/// dispatch path detached, the helper reaped, the control alias closed, and
/// no non-payload alias remaining. Starts fully unsatisfied — every
/// condition must be positively proven, never assumed true by default, so a
/// registry with no evidence source wired up can never mint (R9 fail-closed).
/// Until Task 9 (F-8) supplies the real executor-driven setters, only the
/// `#[cfg(test)]` setters below (`close_fake_control` etc.) can satisfy it.
#[derive(Debug, Default)]
pub(crate) struct FamilyInventory {
    submitters_detached: bool,
    helper_reaped: bool,
    control_closed: bool,
    non_payload_aliases: usize,
}

#[allow(dead_code)]
pub(crate) struct DrmCleanupRegistry {
    device_key: DrmDeviceKey,
    incarnation: IncarnationId,
    io: Box<dyn CleanupIo>,
    device: Option<Rc<Device>>,
    frozen: bool,
    family_closed: bool,
    /// Outstanding file-owned payload aliases, keyed by the entry that holds
    /// them. `register_payload_alias` records one at adoption;
    /// `try_mint_file_family_closed` discharges and unregisters each in turn
    /// rather than waiting for it to drop on its own (R5).
    payload_alias_keys: BTreeSet<AllocationKey>,
    /// Keyless payloads whose cleanup failed. These entries own the payload,
    /// its device alias and the already-held role charge until R3 retry or
    /// the incarnation's family-close handoff.
    pending_cleanup: Vec<PendingCleanupEntry>,
    family_inventory: FamilyInventory,
    returned_descriptors: Vec<std::os::fd::OwnedFd>,
    /// Spec 4.1: set once pool-husk accounting can no longer be trusted --
    /// a registration dropped undischarged, or a foreign or unknown one
    /// presented. Shared with every `PoolHuskRegistration` this registry
    /// mints, whose pointer identity is also what proves a registration was
    /// minted here. Never cleared.
    husk_accounting_failed: Rc<Cell<bool>>,
}

impl std::fmt::Debug for DrmCleanupRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DrmCleanupRegistry")
            .field("device_key", &self.device_key)
            .field("incarnation", &self.incarnation)
            .field("has_device", &self.device.is_some())
            .field("frozen", &self.frozen)
            .field("family_closed", &self.family_closed)
            .field("payload_aliases", &self.payload_aliases())
            .finish()
    }
}

#[allow(dead_code)]
impl DrmCleanupRegistry {
    pub(crate) fn new(
        device: Rc<Device>,
        device_key: DrmDeviceKey,
        incarnation: IncarnationId,
    ) -> Self {
        let io = Box::new(DeviceCleanupIo::new(Rc::clone(&device)));
        Self {
            device_key,
            incarnation,
            io,
            device: Some(device),
            frozen: false,
            family_closed: false,
            payload_alias_keys: BTreeSet::new(),
            pending_cleanup: Vec::new(),
            family_inventory: FamilyInventory::default(),
            returned_descriptors: Vec::new(),
            husk_accounting_failed: Rc::new(Cell::new(false)),
        }
    }

    pub(crate) fn new_with_io(
        device_key: DrmDeviceKey,
        incarnation: IncarnationId,
        io: Box<dyn CleanupIo>,
    ) -> Self {
        Self {
            device_key,
            incarnation,
            io,
            device: None,
            frozen: false,
            family_closed: false,
            payload_alias_keys: BTreeSet::new(),
            pending_cleanup: Vec::new(),
            family_inventory: FamilyInventory::default(),
            returned_descriptors: Vec::new(),
            husk_accounting_failed: Rc::new(Cell::new(false)),
        }
    }

    pub(crate) fn new_with_device_and_io(
        device: Rc<Device>,
        device_key: DrmDeviceKey,
        incarnation: IncarnationId,
        io: Box<dyn CleanupIo>,
    ) -> Self {
        Self {
            device_key,
            incarnation,
            io,
            device: Some(device),
            frozen: false,
            family_closed: false,
            payload_alias_keys: BTreeSet::new(),
            pending_cleanup: Vec::new(),
            family_inventory: FamilyInventory::default(),
            returned_descriptors: Vec::new(),
            husk_accounting_failed: Rc::new(Cell::new(false)),
        }
    }

    pub(crate) fn device_key(&self) -> DrmDeviceKey {
        self.device_key
    }

    pub(crate) fn incarnation(&self) -> IncarnationId {
        self.incarnation
    }

    pub(crate) fn is_frozen(&self) -> bool {
        self.frozen
    }

    pub(crate) fn is_family_closed(&self) -> bool {
        self.family_closed
    }

    /// F8-M2: lets a router's own teardown step decide when it is safe to
    /// close returned descriptors (`HandoffRouter::service`) -- once the
    /// helper is reaped, no more late replies can arrive on this incident,
    /// so whatever the router has accumulated so far may be closed without
    /// racing a future `deliver_descriptor`.
    pub(crate) fn helper_reaped(&self) -> bool {
        self.family_inventory.helper_reaped
    }

    pub(crate) fn register_right(
        &mut self,
        fb: u32,
        gem: u32,
        gem_owner: GemOwner,
    ) -> DrmCleanupRight {
        DrmCleanupRight::new(self.device_key, self.incarnation, fb, gem, gem_owner)
    }

    /// Records that `key`'s entry holds a counted alias of this incarnation's
    /// open file description (adoption of a `GbmDevice`/right pair). The
    /// barrier does not wait for this to drop; it discharges and unregisters
    /// it during `try_mint_file_family_closed`.
    pub(crate) fn register_payload_alias(&mut self, key: AllocationKey) {
        self.payload_alias_keys.insert(key);
    }

    pub(crate) fn unregister_payload_alias(&mut self, key: AllocationKey) {
        self.payload_alias_keys.remove(&key);
    }

    pub(crate) fn payload_aliases(&self) -> usize {
        self.payload_alias_keys.len() + self.pending_cleanup.len()
    }

    pub(crate) fn pending_cleanup_entries(&self) -> usize {
        self.pending_cleanup.len()
    }

    pub(crate) fn pending_cleanup_roles(&self) -> Vec<DirectRole> {
        self.pending_cleanup
            .iter()
            .map(|entry| entry.charge.role())
            .collect()
    }

    /// Transfers a payload and its existing charge into the keyless pending
    /// owner. This operation is intentionally infallible for a payload that
    /// came from the production adoption path: failing here would leave no
    /// legal owner for the cleanup right.
    pub(crate) fn retain_pending_cleanup(
        &mut self,
        payload: AllocationPayload,
        charge: CleanupCharge,
    ) {
        self.pending_cleanup
            .push(PendingCleanupEntry { payload, charge });
    }

    pub(crate) fn freeze_incarnation(&mut self) {
        self.frozen = true;
    }

    /// R3: retry every pending payload while this incarnation is live. A
    /// failed retry keeps the returned right in the same entry, so a partial
    /// RMFB/GEM_CLOSE sequence resumes at the right state without a second
    /// cleanup owner.
    pub(crate) fn retry_pending_cleanup(&mut self) -> io::Result<usize> {
        if self.frozen || self.family_closed {
            return Ok(0);
        }

        let pending = std::mem::take(&mut self.pending_cleanup);
        let mut retained = Vec::with_capacity(pending.len());
        let mut released = 0;
        for mut entry in pending {
            match entry.payload.discharge_file_owned(self) {
                Ok(()) => {
                    entry.charge.release();
                    released += 1;
                }
                Err(_) => retained.push(entry),
            }
        }
        self.pending_cleanup = retained;
        Ok(released)
    }

    pub(crate) fn consume(
        &mut self,
        mut right: DrmCleanupRight,
    ) -> Result<(), (io::Error, DrmCleanupRight)> {
        if right.state == RightState::Closed {
            return Ok(());
        }
        if self.frozen || right.state == RightState::Frozen {
            right.state = RightState::Frozen;
            return Err((io::Error::other("incarnation is frozen"), right));
        }

        if self.family_closed {
            return Err((
                io::Error::new(io::ErrorKind::PermissionDenied, "file family is closed"),
                right,
            ));
        }

        if right.state == RightState::Registered {
            if let Err(err) = self.io.remove_fb(right.fb) {
                return Err((err, right));
            }
            right.state = RightState::FramebufferRemoved;
        }

        if right.state == RightState::FramebufferRemoved {
            let res = if right.gem_owner == GemOwner::Right {
                self.io.close_gem(right.gem)
            } else {
                Ok(())
            };
            if let Err(err) = res {
                return Err((err, right));
            }
            // GemOwner::Gbm never closes the GEM handle in DrmCleanupRight
            right.state = RightState::Discharged;
            let _ = right;
        }

        Ok(())
    }

    /// Resets `family_inventory` to fully unsatisfied. Only ever needed by a
    /// test that reuses one registry across more than one gating scenario;
    /// every constructor already starts from `FamilyInventory::default()`.
    #[cfg(test)]
    pub(crate) fn init_fake_family(&mut self) {
        self.family_inventory = FamilyInventory::default();
    }

    #[cfg(test)]
    pub(crate) fn detach_fake_submitters(&mut self) {
        self.family_inventory.submitters_detached = true;
    }

    #[cfg(test)]
    pub(crate) fn close_fake_control(&mut self) {
        self.family_inventory.control_closed = true;
    }

    #[cfg(test)]
    pub(crate) fn reap_fake_helper(&mut self) {
        self.family_inventory.helper_reaped = true;
    }

    #[cfg(test)]
    pub(crate) fn add_fake_alias(&mut self) {
        self.family_inventory.non_payload_aliases += 1;
    }

    #[cfg(test)]
    pub(crate) fn remove_fake_alias(&mut self) {
        self.family_inventory.non_payload_aliases =
            self.family_inventory.non_payload_aliases.saturating_sub(1);
    }

    pub(crate) fn register_returned_descriptor(&mut self, fd: std::os::fd::OwnedFd) {
        self.family_inventory.non_payload_aliases += 1;
        self.returned_descriptors.push(fd);
    }

    pub(crate) fn close_returned_descriptors(&mut self) {
        let count = self.returned_descriptors.len();
        self.returned_descriptors.clear();
        self.family_inventory.non_payload_aliases = self
            .family_inventory
            .non_payload_aliases
            .saturating_sub(count);
    }

    /// Takes the husk's device alias by value (round-1 B-1): the
    /// registration owns it from here, and only consuming the registration
    /// releases it.
    pub(crate) fn register_pool_husk(
        &mut self,
        alias: Rc<crate::drm::Device>,
    ) -> PoolHuskRegistration {
        self.family_inventory.non_payload_aliases += 1;
        PoolHuskRegistration {
            device_key: self.device_key,
            incarnation: self.incarnation,
            accounting: Rc::clone(&self.husk_accounting_failed),
            alias: Some(alias),
            discharged: false,
        }
    }

    /// Spec 4.1: uncounts the husk `registration` proves this registry
    /// counted. A registration for another device or incarnation, or one
    /// another registry minted, is refused and fails closed on both sides:
    /// this registry refuses to mint from now on, and so does the one that
    /// minted it, when the refused registration drops undischarged.
    pub(crate) fn unregister_pool_husk(
        &mut self,
        mut registration: PoolHuskRegistration,
    ) -> Result<(), ResourceError> {
        if registration.device_key != self.device_key
            || registration.incarnation != self.incarnation
        {
            self.husk_accounting_failed.set(true);
            return Err(ResourceError::WrongIncarnation);
        }
        if !Rc::ptr_eq(&registration.accounting, &self.husk_accounting_failed) {
            self.husk_accounting_failed.set(true);
            return Err(ResourceError::InvalidProof);
        }
        let Some(remaining) = self.family_inventory.non_payload_aliases.checked_sub(1) else {
            self.husk_accounting_failed.set(true);
            return Err(ResourceError::InvalidState);
        };
        self.family_inventory.non_payload_aliases = remaining;
        registration.discharged = true;
        // The alias this registration accounted for ends here, with the
        // count that named it (round-1 B-1).
        drop(registration.alias.take());
        Ok(())
    }

    /// Becomes mintable when every submitter is detached, the helper is
    /// reaped, the control alias is closed and no non-payload alias remains
    /// (R5). These are real preconditions, checked unconditionally in every
    /// build: `family_inventory` starts fully unsatisfied, so a registry
    /// with no evidence source wired up can never mint (R9 fail-closed) --
    /// until Task 9 (F-8) supplies real executor-driven setters, only the
    /// `#[cfg(test)]` setters above can satisfy it. Outstanding payload
    /// aliases are never a precondition — an `Rc` reaching zero does not
    /// establish the barrier (Global Constraints) and a payload merely
    /// waiting for it while holding its alias is the leak the design
    /// forbids. Instead, once the above conditions hold, the registry walks
    /// its own inventory of outstanding file-owned contexts and discharges
    /// each through `discharge_payload_alias` — supplied by the caller
    /// because the registry owns the closing order but the service owns the
    /// payloads — unregistering as each one closes, then drops its own alias
    /// and mints.
    pub(crate) fn try_mint_file_family_closed(
        &mut self,
        mut discharge_payload_alias: impl FnMut(&mut Self, AllocationKey) -> Result<(), io::Error>,
    ) -> Result<FileFamilyClosed, io::Error> {
        if self.family_closed {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "file family already closed",
            ));
        }

        if !self.family_inventory.submitters_detached {
            return Err(io::Error::other("submitters are not detached"));
        }
        if !self.family_inventory.helper_reaped {
            return Err(io::Error::other("helper process is not reaped"));
        }
        if !self.family_inventory.control_closed {
            return Err(io::Error::other("control fd is not closed"));
        }
        // Spec 4.1: checked before the alias count, so a refusal says which
        // of the two it is -- a dropped registration also leaves its alias
        // counted.
        if self.husk_accounting_failed.get() {
            return Err(io::Error::other("pool husk accounting failed"));
        }
        if self.family_inventory.non_payload_aliases > 0 {
            return Err(io::Error::other("non-payload aliases still active"));
        }

        let keys: Vec<AllocationKey> = self.payload_alias_keys.iter().copied().collect();
        for key in keys {
            discharge_payload_alias(self, key)?;
            self.payload_alias_keys.remove(&key);
        }

        // Pending entries have already failed a live cleanup and the family
        // is now closing. Their DRM objects died with the family, so mark the
        // right closed without issuing a stale ioctl, release the original
        // charge, and drop the now-safe payload.
        let pending = std::mem::take(&mut self.pending_cleanup);
        for mut entry in pending {
            entry.payload.close_file_owned_after_family();
            entry.charge.release();
        }

        // Registry performs the description's last close
        self.device = None;
        self.family_closed = true;

        Ok(FileFamilyClosed {
            device_key: self.device_key,
            incarnation: self.incarnation,
            _private: (),
        })
    }

    pub(crate) fn retire_closed_family(
        &mut self,
        proof: FileFamilyClosed,
    ) -> Result<(), ResourceError> {
        if proof.device_key != self.device_key || proof.incarnation != self.incarnation {
            return Err(ResourceError::WrongIncarnation);
        }
        self.device = None;
        self.family_closed = true;
        Ok(())
    }
}

#[allow(dead_code)]
pub(crate) struct DirectFramebufferAllocation {
    pub(crate) right: Option<DrmCleanupRight>,
    pub(crate) source_lease: Option<AllocationLease>,
    pub(crate) fb: framebuffer::Handle,
    pub(crate) gem: DrmBufferHandle,
    pub(crate) gem_owner: GemOwner,
    pub(crate) device: Option<Rc<Device>>,
}

impl std::fmt::Debug for DirectFramebufferAllocation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DirectFramebufferAllocation")
            .field("right", &self.right)
            .field("source_lease", &self.source_lease)
            .field("fb", &self.fb)
            .field("gem", &self.gem)
            .field("gem_owner", &self.gem_owner)
            .field("has_device", &self.device.is_some())
            .finish()
    }
}

#[allow(dead_code)]
impl DirectFramebufferAllocation {
    pub(crate) fn new(
        right: DrmCleanupRight,
        source_lease: Option<AllocationLease>,
        fb: framebuffer::Handle,
        gem: DrmBufferHandle,
        gem_owner: GemOwner,
        device: Option<Rc<Device>>,
    ) -> Self {
        Self {
            right: Some(right),
            source_lease,
            fb,
            gem,
            gem_owner,
            device,
        }
    }

    pub(crate) fn take_right(&mut self) -> Option<DrmCleanupRight> {
        self.right.take()
    }

    pub(crate) fn right(&self) -> Option<&DrmCleanupRight> {
        self.right.as_ref()
    }

    pub(crate) fn fb_handle(&self) -> framebuffer::Handle {
        self.fb
    }

    pub(crate) fn gem_handle(&self) -> DrmBufferHandle {
        self.gem
    }

    pub(crate) fn source_lease(&self) -> Option<&AllocationLease> {
        self.source_lease.as_ref()
    }

    /// Discharges this payload's file-owned half: consumes the right
    /// (RMFB/GEM_CLOSE per its `GemOwner`) then drops the counted device
    /// alias. On failure the right is put back so a retry can resume from
    /// `FramebufferRemoved` per the R3 state machine; the device alias is
    /// only ever dropped once the right has fully discharged.
    pub(crate) fn discharge_file_owned(
        &mut self,
        registry: &mut DrmCleanupRegistry,
    ) -> Result<(), io::Error> {
        if let Some(right) = self.right.take() {
            match registry.consume(right) {
                Ok(()) => {}
                Err((err, returned_right)) => {
                    self.right = Some(returned_right);
                    return Err(err);
                }
            }
        }
        self.device = None;
        Ok(())
    }

    pub(crate) fn close_file_owned_after_family(&mut self) {
        if let Some(right) = self.right.as_mut() {
            right.mark_closed();
        }
        self.device = None;
    }
}
