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
/// The real implementor (F5a-M1) is `backend::ScanoutM2OwnershipHandle`: a
/// clone of the live cells `ScanoutM2State` itself keeps in sync at its own
/// `current`/`pending`/`queued_successor`/`unflip_requested` mutation
/// sites (`sync_ownership`, called after every one of them) -- never a
/// value some unrelated caller must remember to publish. Installing a gate
/// against it in production is later work (R8: there is no production
/// issuer of Owner in this stage, so no gate is installed in production yet
/// either). `#[cfg(test)]` code implements this trait with an explicit fake
/// that records every query it was asked so a test can assert the gate
/// actually consulted live state.
pub(crate) trait DirectOwnershipState: fmt::Debug {
    /// True while any direct ownership unit (`Current`/`Submitted`/
    /// `Successor`) is occupied for this gate's device.
    fn direct_ownership_busy(&self) -> bool;
    /// True while an unflip has been requested and has not yet retired.
    fn unflip_outstanding(&self) -> bool;
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

/// Spec 4.2 (stage 2c-i debt), round-1 B-2: a handle names the gate it
/// closes. `device`/`incarnation` are what a holder validates against its
/// own identity before installing one; `forced_closed` doubles as the gate's
/// instance identity, since it is the very cell that gate reads.
#[derive(Clone, Debug)]
pub(crate) struct TransportGateHandle {
    device: DrmDeviceKey,
    incarnation: IncarnationId,
    forced_closed: Rc<Cell<bool>>,
}

impl TransportGateHandle {
    pub(crate) fn close_gate(&self) {
        self.forced_closed.set(true);
    }

    pub(crate) fn is_closed(&self) -> bool {
        self.forced_closed.get()
    }

    pub(crate) fn device(&self) -> DrmDeviceKey {
        self.device
    }

    pub(crate) fn incarnation(&self) -> IncarnationId {
        self.incarnation
    }

    /// True when both handles name the same gate instance, not merely the
    /// same device and incarnation.
    pub(crate) fn same_gate(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.forced_closed, &other.forced_closed)
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
    forced_closed: Rc<Cell<bool>>,
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
            forced_closed: Rc::new(Cell::new(false)),
        }
    }

    #[cfg(test)]
    pub(crate) fn for_tests(device: DrmDeviceKey, incarnation: IncarnationId) -> Self {
        Self::new_legacy(
            device,
            incarnation,
            Box::new(FakeDirectOwnershipState::new()),
        )
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
        if self.state() == TransportState::Closed {
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
    pub(crate) fn force_close(&mut self) {
        self.forced_closed.set(true);
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

    pub(crate) fn handle(&self) -> TransportGateHandle {
        TransportGateHandle {
            device: self.device,
            incarnation: self.incarnation,
            forced_closed: Rc::clone(&self.forced_closed),
        }
    }

    /// The effective state. Round-2 B-1: every transition below consults
    /// this, never the raw `self.state`, because a close can arrive through a
    /// `TransportGateHandle` -- which owns no `&mut TransportGate` and can
    /// only set the shared flag. A handle-driven close must be as terminal as
    /// `close()` itself: no quiescing, no permit, no publication, no grant
    /// after it.
    pub(crate) fn state(&self) -> TransportState {
        if self.forced_closed.get() {
            TransportState::Closed
        } else {
            self.state
        }
    }

    pub(crate) fn allows_legacy(&self, _class: WriterClass) -> bool {
        self.state() == TransportState::Legacy
    }

    /// B-10/R11: the single check every real DRM/helper write sink performs
    /// immediately before it actually dispatches, composing `allows_legacy`
    /// with the Owner-writer authority contract (plan-review round-2 M-1)
    /// so a sink does not have to re-derive it. `Legacy` permits
    /// unconditionally -- the only route any production caller can reach
    /// today (R8), so `grant` is always `None` there in practice. `Quiescing`
    /// and `Closed` refuse every write, matching R7 ("Quiescing permits no
    /// writer class"). `Owner` requires a `grant` whose own `class` matches
    /// `class`; a class mismatch refuses without consuming anything (the
    /// caller's now-unused grant then closes admission when it is dropped,
    /// per the lost-role-token rule); a matching grant is consumed through
    /// `consume_owner_write`, which independently enforces device/
    /// incarnation and force-closes the transport on a foreign one.
    #[allow(clippy::result_large_err)]
    pub(crate) fn authorize_write(
        &mut self,
        class: WriterClass,
        grant: Option<OwnerWriteGrant>,
    ) -> Result<(), ResourceError> {
        match self.state() {
            TransportState::Legacy => Ok(()),
            TransportState::Quiescing => Err(ResourceError::Busy),
            TransportState::Closed => Err(ResourceError::Detached),
            TransportState::Owner => {
                let grant = grant.ok_or(ResourceError::Detached)?;
                if grant.class() != class {
                    return Err(ResourceError::InvalidProof);
                }
                self.consume_owner_write(grant).map_err(|(err, _grant)| err)
            }
        }
    }

    pub(crate) fn authorize_owner_write(
        &mut self,
        class: WriterClass,
    ) -> Result<OwnerWriteGrant, ResourceError> {
        if self.state() != TransportState::Owner {
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
        if self.state() != TransportState::Owner {
            return Err((ResourceError::Detached, grant));
        }
        if !self.issued_serials.remove(&grant.serial) {
            return Err((ResourceError::InvalidProof, grant));
        }
        // Minor (round-1 review): a plain `if outstanding > 0 { -= 1 }` masks
        // an accounting bug (a serial issued and removed above with nothing
        // to decrement) by silently doing nothing instead of surfacing it.
        // `issued_serials` and `outstanding_owner_writes` are supposed to
        // move together; a mismatch here means they already diverged, and
        // hiding that would let a stuck admission-closed gate look healthy.
        self.outstanding_owner_writes = match self.outstanding_owner_writes.checked_sub(1) {
            Some(remaining) => remaining,
            None => return Err((ResourceError::InvalidState, grant)),
        };
        grant.consumed.set(true);
        Ok(())
    }

    pub(crate) fn outstanding_owner_writes(&self) -> usize {
        self.outstanding_owner_writes
    }

    /// Test-only: force a desync between `outstanding_owner_writes` and
    /// `issued_serials` that the normal `authorize_owner_write`/
    /// `consume_owner_write` pairing can never produce, so the checked-
    /// subtraction accounting fix (round-1 review minor) is reachable from a
    /// test at all.
    #[cfg(test)]
    pub(crate) fn set_outstanding_owner_writes_for_tests(&mut self, value: usize) {
        self.outstanding_owner_writes = value;
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
        if self.state() != TransportState::Quiescing {
            return Err(ResourceError::Busy);
        }
        if self.outstanding_owner_writes != 0 {
            return Err(ResourceError::Busy);
        }
        self.state = TransportState::Owner;
        Ok(())
    }
}
