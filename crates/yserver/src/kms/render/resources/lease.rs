use std::{
    cell::RefCell,
    collections::BTreeSet,
    rc::{Rc, Weak},
};

use super::availability::{AllocationEntry, AllocationKey, UseId, UseKind};

#[allow(dead_code)]
#[derive(Debug)]
pub(crate) struct AllocationLease {
    pub(in crate::kms::render::resources) entry: Rc<AllocationEntry>,
    pub(crate) use_id: UseId,
    pub(crate) kind: UseKind,
    pub(crate) dirty_queue: Weak<RefCell<BTreeSet<AllocationKey>>>,
    pub(crate) zero_edge_queue: Weak<RefCell<BTreeSet<AllocationKey>>>,
}

#[allow(dead_code)]
impl AllocationLease {
    pub(in crate::kms::render::resources) fn new(
        entry: Rc<AllocationEntry>,
        use_id: UseId,
        kind: UseKind,
        dirty_queue: Weak<RefCell<BTreeSet<AllocationKey>>>,
        zero_edge_queue: Weak<RefCell<BTreeSet<AllocationKey>>>,
    ) -> Self {
        Self {
            entry,
            use_id,
            kind,
            dirty_queue,
            zero_edge_queue,
        }
    }

    pub(crate) fn key(&self) -> AllocationKey {
        self.entry.key()
    }

    pub(crate) fn use_id(&self) -> UseId {
        self.use_id
    }

    pub(crate) fn kind(&self) -> UseKind {
        self.kind
    }
}

impl Drop for AllocationLease {
    fn drop(&mut self) {
        self.entry.remove_use(self.use_id);
        if self.kind == UseKind::DirectFramebuffer
            && self.entry.live_use_count() == 0
            && self.entry.payload.borrow().as_ref().is_some_and(|payload| {
                matches!(payload, super::AllocationPayload::DirectFramebuffer(_))
            })
            && let Some(zero_edges) = self.zero_edge_queue.upgrade()
        {
            zero_edges.borrow_mut().insert(self.entry.key());
        }
        if let Some(dirty) = self.dirty_queue.upgrade() {
            dirty.borrow_mut().insert(self.entry.key());
        }
    }
}
