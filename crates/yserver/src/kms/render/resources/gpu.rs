#![allow(dead_code)]
use std::{cell::Cell, fmt, sync::Arc};

use crate::kms::{
    render::{
        platform::FenceTicket,
        resources::{AllocationKey, AllocationLease, ObligationId},
    },
    vk::device::VkContext,
};

/// A GPU obligation tracking pending execution for allocations covered by a submission fence.
/// Not Send.
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
            .field("has_context", &self.context.is_some())
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
pub(crate) struct CoreRetirementBatch {
    pub(crate) leases: Vec<AllocationLease>,
    pub(crate) descriptor_slots: Vec<usize>,
    pub(crate) obligation: Option<GpuObligation>,
    pub(crate) read_obligation: Option<ReadObligation>,
    pub(crate) possibly_dispatched: bool,
    /// Test hook for controlled mock ticket status query.
    #[cfg(test)]
    pub(crate) test_ticket_status: Option<Result<bool, ash::vk::Result>>,
    /// Destruction counter probe for testing.
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
            .finish()
    }
}

impl CoreRetirementBatch {
    pub(crate) fn new(
        leases: Vec<AllocationLease>,
        descriptor_slots: Vec<usize>,
        possibly_dispatched: bool,
    ) -> Self {
        Self {
            leases,
            descriptor_slots,
            obligation: None,
            read_obligation: None,
            possibly_dispatched,
            #[cfg(test)]
            test_ticket_status: None,
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

        if let Some(context) = &ob.context {
            ob.ticket.poll_signaled_result(context)
        } else {
            ob.ticket.poll_signaled_result_opt(None)
        }
    }

    pub(crate) fn obligation(&self) -> Option<&GpuObligation> {
        self.obligation.as_ref()
    }

    pub(crate) fn leases(&self) -> &[AllocationLease] {
        &self.leases
    }

    pub(crate) fn descriptor_slots(&self) -> &[usize] {
        &self.descriptor_slots
    }
}

impl Drop for CoreRetirementBatch {
    fn drop(&mut self) {
        if let Some(counter) = &self.drop_counter {
            counter.set(counter.get() + 1);
        }
    }
}

/// Private validated GPU batch ready for serialized, infallible application.
pub(crate) struct ValidatedGpuBatch {
    pub(crate) batch: CoreRetirementBatch,
    pub(crate) confirmed_entries: Vec<(AllocationKey, ObligationId)>,
}
