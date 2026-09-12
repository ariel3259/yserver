#![allow(dead_code)]
use std::{cell::Cell, collections::BTreeSet, fmt, rc::Rc};

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
    // B-6/F4: this was briefly `#[doc(hidden)] pub(crate)` so that a
    // non-test `RetainingSupervisor::reserve_slot` would compile -- that
    // was the bug (R8: no production `RecipientReservation`), not a
    // reason to keep the constructor reachable outside tests.
    // `RetainingSupervisor` (and the `reserve_slot` that called this) is
    // back under `#[cfg(test)]` in `handoff.rs`, so this constructor can
    // live under `#[cfg(test)]` again with no production caller needing it.
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
#[derive(Debug)]
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

/// Real direct-ownership and unflip state that `begin_quiescing` consults
/// for its R7 precondition (M-13). Supplied to the gate once, at
/// construction: `TransportGate`'s own public surface no longer has any
/// setter that can flip Busy directly (`set_direct_scanout_active` /
/// `set_unflip_pending` are gone -- they had no non-test caller and could
/// be driven by anything in the crate). Only whoever implements this trait
/// over its own real state can influence the answer, and the gate queries
/// it live on every `begin_quiescing` call rather than caching a value.
///
/// The real backend owner is a small adapter over its own authoritative
/// state (e.g. the existing direct-scanout `current`/`pending`/
/// `queued_successor` occupancy and its `unflip_requested` flag) -- wiring
/// that adapter's update calls into the real scanout/unflip transition
/// sites, and installing a gate against it in production, is later work
/// (R8: there is no production issuer of Owner in this stage, so no gate is
/// installed in production yet either). `#[cfg(test)]` code implements this
/// trait with an explicit fake that records every query it was asked so a
/// test can assert the gate actually consulted live state.
pub(crate) trait DirectOwnershipState: fmt::Debug {
    /// True while any direct ownership unit (`Current`/`Submitted`/
    /// `Successor`) is occupied for this gate's device.
    fn direct_ownership_busy(&self) -> bool;
    /// True while an unflip has been requested and has not yet retired.
    fn unflip_outstanding(&self) -> bool;
}

/// Shared live cells a real owner can publish into and keep a clone of, so
/// it can push its own authoritative transitions in without exposing any
/// setter on `TransportGate` itself. Not `#[cfg(test)]`: this is the "the
/// backend implements it" half of M-13's contract -- a real, correctly
/// typed adapter over shared state, ready for a later stage to construct
/// from its own direct-scanout/unflip bookkeeping and keep updated at its
/// own mutation sites. Nothing in this crate installs one against a
/// production `TransportGate` yet (R8), so it has no non-test caller today,
/// exactly like `OwnerWriteGrant`'s issuer and `RecipientReservation`.
#[derive(Debug, Clone, Default)]
pub(crate) struct DirectOwnershipSignal {
    busy: Rc<Cell<bool>>,
    unflip_outstanding: Rc<Cell<bool>>,
}

impl DirectOwnershipSignal {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Publishes the real direct-ownership occupancy the owner just
    /// observed (e.g. `any of Current/Submitted/Successor occupied`).
    pub(crate) fn set_direct_ownership_busy(&self, busy: bool) {
        self.busy.set(busy);
    }

    /// Publishes the real unflip-lifecycle state the owner just observed
    /// (`requested and not yet retired`).
    pub(crate) fn set_unflip_outstanding(&self, outstanding: bool) {
        self.unflip_outstanding.set(outstanding);
    }
}

impl DirectOwnershipState for DirectOwnershipSignal {
    fn direct_ownership_busy(&self) -> bool {
        self.busy.get()
    }

    fn unflip_outstanding(&self) -> bool {
        self.unflip_outstanding.get()
    }
}

/// Test-only fake (M-13/F4): records how many times each query was asked,
/// alongside the answer to give, so a test can assert `begin_quiescing`
/// actually consulted live state rather than a value cached at
/// construction. Never reachable outside `#[cfg(test)]` code.
#[cfg(test)]
#[derive(Debug, Clone, Default)]
pub(crate) struct FakeDirectOwnershipState {
    busy: Rc<Cell<bool>>,
    unflip_outstanding: Rc<Cell<bool>>,
    busy_queries: Rc<Cell<usize>>,
    unflip_queries: Rc<Cell<usize>>,
}

#[cfg(test)]
impl FakeDirectOwnershipState {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn set_busy(&self, busy: bool) {
        self.busy.set(busy);
    }

    pub(crate) fn set_unflip_outstanding(&self, outstanding: bool) {
        self.unflip_outstanding.set(outstanding);
    }

    pub(crate) fn busy_query_count(&self) -> usize {
        self.busy_queries.get()
    }

    pub(crate) fn unflip_query_count(&self) -> usize {
        self.unflip_queries.get()
    }
}

#[cfg(test)]
impl DirectOwnershipState for FakeDirectOwnershipState {
    fn direct_ownership_busy(&self) -> bool {
        self.busy_queries.set(self.busy_queries.get() + 1);
        self.busy.get()
    }

    fn unflip_outstanding(&self) -> bool {
        self.unflip_queries.set(self.unflip_queries.get() + 1);
        self.unflip_outstanding.get()
    }
}

#[derive(Debug)]
pub(crate) struct TransportGate {
    device: DrmDeviceKey,
    incarnation: IncarnationId,
    state: TransportState,
    ownership: Box<dyn DirectOwnershipState>,
    outstanding_owner_writes: usize,
    next_serial: u64,
    issued_serials: BTreeSet<u64>,
    closed_admission: Rc<Cell<bool>>,
}

impl TransportGate {
    pub(crate) fn new_legacy(
        device: DrmDeviceKey,
        incarnation: IncarnationId,
        ownership: Box<dyn DirectOwnershipState>,
    ) -> Self {
        Self {
            device,
            incarnation,
            state: TransportState::Legacy,
            ownership,
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

    /// R7 precondition: `Busy` while any direct ownership unit is
    /// `Current`/`Submitted`/`Successor`, or while an unflip is requested
    /// and not retired -- read live from the real state given at
    /// construction (M-13), never from a setter on this gate.
    pub(crate) fn begin_quiescing(&mut self) -> Result<(), ResourceError> {
        if self.state == TransportState::Closed {
            return Err(ResourceError::Detached);
        }
        if self.ownership.direct_ownership_busy()
            || self.ownership.unflip_outstanding()
            || self.outstanding_owner_writes != 0
        {
            return Err(ResourceError::Busy);
        }
        self.state = TransportState::Quiescing;
        Ok(())
    }

    /// Unconditional closure used only for the foreign-proof emergency path
    /// (a mismatched `consume_owner_write` must close immediately,
    /// regardless of any other outstanding grant -- that grant's owner is
    /// already quarantined by the same mismatch). The public `close` below
    /// is the graceful path and refuses while grants are outstanding
    /// (M-14).
    fn force_close(&mut self) {
        self.state = TransportState::Closed;
    }

    /// M-14: refuses while `outstanding_owner_writes() != 0` -- closing
    /// (and thereby the handover it precedes) must not silently orphan a
    /// dispatch whose outcome is still unknown.
    #[allow(clippy::result_large_err)]
    pub(crate) fn close(&mut self) -> Result<(), ResourceError> {
        if self.outstanding_owner_writes != 0 {
            return Err(ResourceError::Busy);
        }
        self.force_close();
        Ok(())
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
            self.force_close();
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

    /// M-14: takes the real `LegacyDrained` proof the backend obtained from
    /// draining the legacy route, plus the final dispositions of that
    /// drain, rather than trusting a caller to have applied them
    /// beforehand. A proof for a foreign device/incarnation, or any
    /// disposition that shows a backend failure, refuses the permit --
    /// mirroring the same fail-closed rule `try_finish_legacy_transport`
    /// already applies before it ever reaches this call (R9: proofs are
    /// never fabricated, and a malformed one closes the route rather than
    /// being silently accepted).
    #[allow(clippy::result_large_err)]
    pub(crate) fn issue_handover_permit(
        &mut self,
        proof: crate::kms::render::platform::LegacyDrained,
        dispositions: &[crate::kms::render::backend::LegacyEventDisposition],
        _coverage: &WriterCoverageProof,
        _reservation: RecipientReservation,
    ) -> Result<HandoverPermit, ResourceError> {
        if self.state != TransportState::Quiescing {
            return Err(ResourceError::Busy);
        }
        if self.outstanding_owner_writes != 0 {
            return Err(ResourceError::Busy);
        }
        if proof.incarnation != self.incarnation {
            return Err(ResourceError::WrongIncarnation);
        }
        let backend_failure = dispositions.iter().any(|d| {
            matches!(
                d,
                crate::kms::render::backend::LegacyEventDisposition::Cancelled(
                    crate::kms::render::backend::LegacyEventCancellation::BackendFailure
                )
            )
        });
        if backend_failure {
            return Err(ResourceError::InvalidProof);
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
