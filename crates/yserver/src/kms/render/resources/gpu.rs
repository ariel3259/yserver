#![allow(dead_code)]
#[cfg(test)]
use std::cell::Cell;
use std::{fmt, sync::Arc};

use ash::vk;

use crate::kms::{
    render::{
        platform::FenceTicket,
        resources::{AllocationKey, AllocationLease, ObligationId, ResourceError, ResourceService},
    },
    vk::device::VkContext,
};

/// A GPU obligation tracking pending execution for allocations covered by a submission fence.
/// Not Send.
///
/// `context` is `Option<Arc<VkContext>>` (F4-B1, the F5 amendment at
/// F2-m2): every non-test constructor (`new`) still takes `Arc<VkContext>`
/// by value and stores `Some` -- a `GpuObligation` built by production or by
/// a `_vulkan` test exists only once a real submission has returned a real
/// ticket, and its completion can only ever be proven against the real
/// device that issued it. The *only* `None` constructor is
/// `#[cfg(test)] for_tests_stub`, for deterministic tests of the batch
/// machine's logic (atomicity, quarantine, freeze, serviced-time accounting)
/// that drive completion through `CoreRetirementBatch::test_ticket_status`
/// and never reach `poll_signaled_result` at all. A `None` reaching the real
/// poll path is a bug, not a fallback status -- see `ticket_status` below.
pub(crate) struct GpuObligation {
    pub(crate) entries: Vec<(AllocationKey, ObligationId)>,
    pub(crate) ticket: FenceTicket,
    pub(crate) context: Option<Arc<VkContext>>,
}

impl fmt::Debug for GpuObligation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GpuObligation")
            .field("entries", &self.entries)
            .field("ticket", &self.ticket)
            .finish()
    }
}

impl GpuObligation {
    pub(crate) fn new(
        entries: Vec<(AllocationKey, ObligationId)>,
        ticket: FenceTicket,
        context: Arc<VkContext>,
    ) -> Self {
        Self {
            entries,
            ticket,
            context: Some(context),
        }
    }

    /// The only `None`-context constructor (F4-B1). For deterministic tests
    /// of the batch machine's logic that never let a real ticket reach
    /// `poll_signaled_result` -- `CoreRetirementBatch::test_ticket_status`
    /// always intercepts `ticket_status()` first. Not reachable outside
    /// `#[cfg(test)]`, and not a substitute for a live device in any
    /// production or `_vulkan` path.
    #[cfg(test)]
    pub(crate) fn for_tests_stub(
        entries: Vec<(AllocationKey, ObligationId)>,
        ticket: FenceTicket,
    ) -> Self {
        Self {
            entries,
            ticket,
            context: None,
        }
    }

    pub(crate) fn entries(&self) -> &[(AllocationKey, ObligationId)] {
        &self.entries
    }

    pub(crate) fn ticket(&self) -> &FenceTicket {
        &self.ticket
    }

    pub(crate) fn context(&self) -> Option<&Arc<VkContext>> {
        self.context.as_ref()
    }
}

/// A read obligation retaining source/staging allocation uses across synchronous or GPU readback.
/// Not Send.
pub(crate) struct ReadObligation {
    pub(crate) source_lease: AllocationLease,
    pub(crate) source_obligation: ObligationId,
    pub(crate) staging_lease: Option<AllocationLease>,
    pub(crate) staging_obligation: Option<ObligationId>,
}

impl fmt::Debug for ReadObligation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ReadObligation")
            .field("source_key", &self.source_lease.key())
            .field("source_obligation", &self.source_obligation)
            .field("has_staging", &self.staging_lease.is_some())
            .field("staging_obligation", &self.staging_obligation)
            .finish()
    }
}

impl ReadObligation {
    pub(crate) fn new(
        source_lease: AllocationLease,
        source_obligation: ObligationId,
        staging_lease: Option<AllocationLease>,
        staging_obligation: Option<ObligationId>,
    ) -> Self {
        Self {
            source_lease,
            source_obligation,
            staging_lease,
            staging_obligation,
        }
    }

    pub(crate) fn source_key(&self) -> AllocationKey {
        self.source_lease.key()
    }

    pub(crate) fn source_obligation(&self) -> ObligationId {
        self.source_obligation
    }
}

/// Retains command/descriptor slot ownership and all managed leases used by a submission,
/// with `Option<GpuObligation>` set only after submission returns its ticket.
/// Not Send.
///
/// `descriptor_slots` holds the real `vk::DescriptorSet` handles the
/// submission drew from the descriptor ring (M-23/5.6): a bare `usize`
/// index proves nothing about exclusion from reset, since nothing stops the
/// ring from reusing that index for an unrelated set while this batch is
/// still pending. Retaining the actual handle means a caller that keeps the
/// batch alive is holding the same object the reset call would need to
/// touch.
pub(crate) struct CoreRetirementBatch {
    pub(crate) leases: Vec<AllocationLease>,
    pub(crate) descriptor_slots: Vec<vk::DescriptorSet>,
    pub(crate) obligation: Option<GpuObligation>,
    pub(crate) read_obligation: Option<ReadObligation>,
    pub(crate) possibly_dispatched: bool,
    /// B-11: this batch's own serviced-time expiry, an absolute point on
    /// the service's cumulative `serviced_elapsed` timeline computed with
    /// checked arithmetic at registration (`ResourceService::register_batch`)
    /// as `serviced_elapsed_at_registration + max_serviced_duration`. Never
    /// compared against wall-clock time and never a second, independent
    /// `serviced_elapsed` counter: `service_completions` advances the one
    /// service-wide `serviced_elapsed` (paused while the seat is inactive,
    /// R9) and then checks each pending batch's own deadline against it, so
    /// two batches registered at different serviced times expire
    /// independently and expiry quarantines only the batch whose deadline
    /// passed -- never a service-wide `exhausted` flip. `None` means the
    /// checked addition overflowed at registration (unrepresentable
    /// deadline): such a batch is quarantined immediately rather than
    /// polled forever.
    pub(crate) serviced_deadline: Option<std::time::Duration>,
    /// Test hook for controlled mock ticket status query.
    #[cfg(test)]
    pub(crate) test_ticket_status: Option<Result<bool, ash::vk::Result>>,
    /// Destruction counter probe for testing (F4/M-23: test-only, never a
    /// production field -- nothing production-side observes how many times
    /// a batch has dropped).
    #[cfg(test)]
    pub(crate) drop_counter: Option<std::rc::Rc<Cell<usize>>>,
}

impl fmt::Debug for CoreRetirementBatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CoreRetirementBatch")
            .field("leases_count", &self.leases.len())
            .field("descriptor_slots", &self.descriptor_slots)
            .field("has_obligation", &self.obligation.is_some())
            .field("has_read_obligation", &self.read_obligation.is_some())
            .field("possibly_dispatched", &self.possibly_dispatched)
            .field("serviced_deadline", &self.serviced_deadline)
            .finish()
    }
}

impl CoreRetirementBatch {
    pub(crate) fn new(
        leases: Vec<AllocationLease>,
        descriptor_slots: Vec<vk::DescriptorSet>,
        possibly_dispatched: bool,
    ) -> Self {
        Self {
            leases,
            descriptor_slots,
            obligation: None,
            read_obligation: None,
            possibly_dispatched,
            serviced_deadline: None,
            #[cfg(test)]
            test_ticket_status: None,
            #[cfg(test)]
            drop_counter: None,
        }
    }

    pub(crate) fn bind_ticket(&mut self, obligation: GpuObligation) {
        self.obligation = Some(obligation);
        self.possibly_dispatched = true;
    }

    pub(crate) fn bind_read_obligation(&mut self, read_obligation: ReadObligation) {
        self.read_obligation = Some(read_obligation);
    }

    pub(crate) fn ticket_status(&self) -> Result<bool, ash::vk::Result> {
        #[cfg(test)]
        if let Some(status) = self.test_ticket_status {
            return status;
        }

        let Some(ob) = &self.obligation else {
            if self.possibly_dispatched {
                // Dispatched without a ticket is an unrecoverable failure
                return Err(ash::vk::Result::ERROR_UNKNOWN);
            }
            return Ok(true);
        };

        // F5/F4-B1: no status fallback. `test_ticket_status` above is the
        // only legal way to reach this method with `context: None` --
        // production and every `_vulkan` test bind a real ticket through
        // `GpuObligation::new`, which always stores `Some`. A `None` here
        // means a real batch was polled without ever having had a real
        // submission bind it, which is a bug in the caller, not a status to
        // report.
        let context = ob.context.as_ref().expect(
            "GpuObligation.context is None outside #[cfg(test)] for_tests_stub, which always \
             intercepts via test_ticket_status before reaching poll_signaled_result",
        );
        ob.ticket.poll_signaled_result(context)
    }

    pub(crate) fn obligation(&self) -> Option<&GpuObligation> {
        self.obligation.as_ref()
    }

    pub(crate) fn leases(&self) -> &[AllocationLease] {
        &self.leases
    }

    pub(crate) fn descriptor_slots(&self) -> &[vk::DescriptorSet] {
        &self.descriptor_slots
    }
}

impl Drop for CoreRetirementBatch {
    fn drop(&mut self) {
        #[cfg(test)]
        if let Some(counter) = &self.drop_counter {
            counter.set(counter.get() + 1);
        }
    }
}

/// Validated GPU batch ready for serialized, infallible application.
/// `pub(super)`: owned entirely by `mod.rs`'s `validate_gpu_batch` /
/// `commit_gpu_batch` pair (F7) -- nothing outside `resources` constructs
/// or inspects one, and nothing inside `resources` other than its producer
/// and consumer needs to.
pub(super) struct ValidatedGpuBatch {
    pub(super) batch: CoreRetirementBatch,
    pub(super) confirmed_entries: Vec<(AllocationKey, ObligationId)>,
}

/// Producer adapter (Task 5, B-15/R9): correlates the real, already-observed
/// outcome of a source-read submission with its registered read obligation.
///
/// `read_succeeded` must reflect actual evidence the caller already
/// obtained by running the real read (e.g. `read_scanout_region`'s
/// `Ok`/`Err`) -- this function performs no I/O itself and never fabricates
/// completion. On success it applies the real proof; on any failure or
/// uncertainty it fails closed (Global Constraints) by freezing the entry
/// rather than discharging it, so a subsequent release cannot proceed on an
/// unproven premise.
pub(crate) fn record_read_outcome(
    service: &mut ResourceService,
    key: AllocationKey,
    obligation: ObligationId,
    read_succeeded: bool,
) -> Result<(), ResourceError> {
    if read_succeeded {
        service.apply_validated_proof(key, obligation)
    } else {
        service.freeze(key)
    }
}

/// Task 5.3: enumerate every write allocation a submission is about to
/// touch, reserve a use and register its GPU obligation for each *before*
/// the caller may hand any raw handle to the GPU, and hand back both the
/// batch (still with `obligation: None` -- no ticket exists yet) and the
/// `(key, obligation)` pairs to bind once dispatch returns one.
///
/// A failure partway through releases every lease/obligation already
/// reserved for this attempt (nothing partially reserved survives a failed
/// prepare), so the caller is never left holding a half-registered batch.
pub(crate) fn prepare_retirement_batch(
    service: &mut ResourceService,
    writes: &[AllocationKey],
    descriptor_slots: Vec<vk::DescriptorSet>,
) -> Result<(CoreRetirementBatch, Vec<(AllocationKey, ObligationId)>), ResourceError> {
    let mut leases = Vec::with_capacity(writes.len());
    let mut entries: Vec<(AllocationKey, ObligationId)> = Vec::with_capacity(writes.len());

    for &key in writes {
        let reserve = service.reserve(key, super::UseKind::Write);
        let lease = match reserve {
            Ok(lease) => lease,
            Err(err) => {
                cancel_prepared_entries(service, &entries);
                return Err(err);
            }
        };
        match service.register(key, super::ObligationKind::Gpu) {
            Ok(obligation) => {
                entries.push((key, obligation));
                leases.push(lease);
            }
            Err(err) => {
                drop(lease);
                cancel_prepared_entries(service, &entries);
                return Err(err);
            }
        }
    }

    Ok((
        CoreRetirementBatch::new(leases, descriptor_slots, false),
        entries,
    ))
}

fn cancel_prepared_entries(
    service: &mut ResourceService,
    entries: &[(AllocationKey, ObligationId)],
) {
    for &(key, obligation) in entries {
        // Best-effort unwind of a partially-prepared batch: the caller
        // already gets the original error back, so a second error here
        // (the entry having since detached) has nowhere else to go, but it
        // must not be a bare `let _` (F6) -- the entries vector is dropped
        // right after this either way, so logging is the only remaining
        // channel and a silent discard here would hide a real double-cancel
        // bug rather than an expected race.
        if let Err(err) = service.cancel(key, obligation) {
            log::debug!(
                "resources::gpu::cancel_prepared_entries: cancel({key:?}, {obligation:?}) \
                 during unwind: {err:?}"
            );
        }
    }
}

/// Task 5.3: a submission that provably never reached the GPU (a pre-submit
/// failure) cancels every obligation `prepare_retirement_batch` registered
/// for it -- the allocations were never touched, so there is nothing to
/// prove and nothing to wait for.
pub(crate) fn cancel_pre_submit_batch(
    service: &mut ResourceService,
    entries: &[(AllocationKey, ObligationId)],
) -> Result<(), ResourceError> {
    let mut first_err = None;
    for &(key, obligation) in entries {
        if let Err(err) = service.cancel(key, obligation)
            && first_err.is_none()
        {
            first_err = Some(err);
        }
    }
    match first_err {
        Some(err) => Err(err),
        None => Ok(()),
    }
}

/// Task 5.3: a submission whose outcome is uncertain (dispatch may or may
/// not have reached the GPU) freezes every obligation it was about to
/// register, rather than cancelling them -- the allocations must not be
/// reused or destroyed while it is unknown whether the GPU is touching
/// them.
pub(crate) fn freeze_uncertain_batch(
    service: &mut ResourceService,
    entries: &[(AllocationKey, ObligationId)],
) -> Result<(), ResourceError> {
    let mut first_err = None;
    for &(key, _) in entries {
        if let Err(err) = service.freeze(key)
            && first_err.is_none()
        {
            first_err = Some(err);
        }
    }
    match first_err {
        Some(err) => Err(err),
        None => Ok(()),
    }
}
