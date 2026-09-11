#![allow(dead_code)]
use std::{cell::Cell, collections::BTreeSet, rc::Rc};

use crate::{
    kms::{owner::identity::IncarnationId, render::resources::availability::ResourceError},
    platform::drm::DrmDeviceKey,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TransportState {
    Legacy,
    Quiescing,
    Owner,
    Closed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WriterClass {
    Primary,
    Unflip,
    Modeset,
    Dpms,
    Vt,
    Topology,
    Cursor,
    Gamma,
    HelperMutation,
}

/// Single-use authority for one owner-mediated mutating dispatch.
/// Not `Clone`, not `Copy`, no `Default`, no constructor outside `transport`.
#[derive(Debug)]
pub(crate) struct OwnerWriteGrant {
    device: DrmDeviceKey,
    incarnation: IncarnationId,
    class: WriterClass,
    serial: u64,
    consumed: Cell<bool>,
    closed_hook: Option<Rc<Cell<bool>>>,
}

impl OwnerWriteGrant {
    pub(crate) fn device(&self) -> DrmDeviceKey {
        self.device
    }

    pub(crate) fn incarnation(&self) -> IncarnationId {
        self.incarnation
    }

    pub(crate) fn class(&self) -> WriterClass {
        self.class
    }

    pub(crate) fn serial(&self) -> u64 {
        self.serial
    }

    #[cfg(test)]
    pub(crate) fn reconstruct_for_tests(
        device: DrmDeviceKey,
        incarnation: IncarnationId,
        class: WriterClass,
        serial: u64,
    ) -> Self {
        Self {
            device,
            incarnation,
            class,
            serial,
            consumed: Cell::new(false),
            closed_hook: None,
        }
    }
}

impl Drop for OwnerWriteGrant {
    fn drop(&mut self) {
        if !self.consumed.get()
            && let Some(hook) = &self.closed_hook
        {
            hook.set(true);
        }
    }
}

/// Opaque capability representing reservation of the recipient endpoint for Owner publication.
pub(crate) struct RecipientReservation {
    _private: (),
}

impl RecipientReservation {
    #[cfg(test)]
    pub(crate) fn new_for_tests() -> Self {
        Self { _private: () }
    }
}

/// Writer coverage proof demonstrating that all writer classes have coverage defined.
pub(crate) struct WriterCoverageProof {
    _private: (),
}

impl WriterCoverageProof {
    #[cfg(test)]
    pub(crate) fn new_for_tests() -> Self {
        Self { _private: () }
    }
}

/// Non-Clone permit issued privately after drain dispositions, consumed to publish Owner.
pub(crate) struct HandoverPermit {
    device: DrmDeviceKey,
    incarnation: IncarnationId,
}

impl HandoverPermit {
    pub(crate) fn device(&self) -> DrmDeviceKey {
        self.device
    }

    pub(crate) fn incarnation(&self) -> IncarnationId {
        self.incarnation
    }
}

#[derive(Debug)]
pub(crate) struct TransportGate {
    device: DrmDeviceKey,
    incarnation: IncarnationId,
    state: TransportState,
    direct_scanout_active: bool,
    unflip_pending: bool,
    outstanding_owner_writes: usize,
    next_serial: u64,
    issued_serials: BTreeSet<u64>,
    closed_admission: Rc<Cell<bool>>,
}

impl TransportGate {
    pub(crate) fn new_legacy(device: DrmDeviceKey, incarnation: IncarnationId) -> Self {
        Self {
            device,
            incarnation,
            state: TransportState::Legacy,
            direct_scanout_active: false,
            unflip_pending: false,
            outstanding_owner_writes: 0,
            next_serial: 0,
            issued_serials: BTreeSet::new(),
            closed_admission: Rc::new(Cell::new(false)),
        }
    }

    pub(crate) fn device(&self) -> DrmDeviceKey {
        self.device
    }

    pub(crate) fn incarnation(&self) -> IncarnationId {
        self.incarnation
    }

    pub(crate) fn set_direct_scanout_active(&mut self, active: bool) {
        self.direct_scanout_active = active;
    }

    pub(crate) fn set_unflip_pending(&mut self, pending: bool) {
        self.unflip_pending = pending;
    }

    pub(crate) fn begin_quiescing(&mut self) -> Result<(), ResourceError> {
        if self.state == TransportState::Closed {
            return Err(ResourceError::Detached);
        }
        if self.direct_scanout_active || self.unflip_pending || self.outstanding_owner_writes != 0 {
            return Err(ResourceError::Busy);
        }
        self.state = TransportState::Quiescing;
        Ok(())
    }

    pub(crate) fn close(&mut self) {
        self.state = TransportState::Closed;
    }

    pub(crate) fn state(&self) -> TransportState {
        self.state
    }

    pub(crate) fn allows_legacy(&self, _class: WriterClass) -> bool {
        self.state == TransportState::Legacy
    }

    pub(crate) fn authorize_owner_write(
        &mut self,
        class: WriterClass,
    ) -> Result<OwnerWriteGrant, ResourceError> {
        if self.state != TransportState::Owner {
            return Err(ResourceError::Detached);
        }
        if self.closed_admission.get() {
            return Err(ResourceError::Detached);
        }
        self.next_serial = self
            .next_serial
            .checked_add(1)
            .ok_or(ResourceError::Detached)?;
        self.outstanding_owner_writes += 1;
        self.issued_serials.insert(self.next_serial);
        Ok(OwnerWriteGrant {
            device: self.device,
            incarnation: self.incarnation,
            class,
            serial: self.next_serial,
            consumed: Cell::new(false),
            closed_hook: Some(Rc::clone(&self.closed_admission)),
        })
    }

    #[allow(clippy::result_large_err)]
    pub(crate) fn consume_owner_write(
        &mut self,
        grant: OwnerWriteGrant,
    ) -> Result<(), (ResourceError, OwnerWriteGrant)> {
        if grant.device != self.device || grant.incarnation != self.incarnation {
            self.close();
            return Err((ResourceError::WrongIncarnation, grant));
        }
        if self.state != TransportState::Owner {
            return Err((ResourceError::Detached, grant));
        }
        if !self.issued_serials.remove(&grant.serial) {
            return Err((ResourceError::InvalidProof, grant));
        }
        if self.outstanding_owner_writes > 0 {
            self.outstanding_owner_writes -= 1;
        }
        grant.consumed.set(true);
        Ok(())
    }

    pub(crate) fn outstanding_owner_writes(&self) -> usize {
        self.outstanding_owner_writes
    }

    pub(crate) fn revoke_owner_writes(&mut self) -> usize {
        let count = self.outstanding_owner_writes;
        self.outstanding_owner_writes = 0;
        self.issued_serials.clear();
        self.closed_admission.set(false);
        count
    }

    pub(crate) fn issue_handover_permit(
        &mut self,
        _coverage: &WriterCoverageProof,
        _reservation: RecipientReservation,
    ) -> Result<HandoverPermit, ResourceError> {
        if self.state != TransportState::Quiescing {
            return Err(ResourceError::Busy);
        }
        if self.outstanding_owner_writes != 0 {
            return Err(ResourceError::Busy);
        }
        Ok(HandoverPermit {
            device: self.device,
            incarnation: self.incarnation,
        })
    }

    pub(crate) fn publish_owner(&mut self, permit: HandoverPermit) -> Result<(), ResourceError> {
        if permit.device != self.device || permit.incarnation != self.incarnation {
            return Err(ResourceError::WrongIncarnation);
        }
        if self.state != TransportState::Quiescing {
            return Err(ResourceError::Busy);
        }
        if self.outstanding_owner_writes != 0 {
            return Err(ResourceError::Busy);
        }
        self.state = TransportState::Owner;
        Ok(())
    }
}
