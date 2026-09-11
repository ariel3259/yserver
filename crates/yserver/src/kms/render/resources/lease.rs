use std::{
    cell::RefCell,
    collections::BTreeSet,
    rc::{Rc, Weak},
};

use super::availability::{AllocationEntry, AllocationKey, UseId, UseKind};

#[allow(dead_code)]
#[derive(Debug)]
pub(crate) struct AllocationLease {
    pub(crate) entry: Rc<AllocationEntry>,
    pub(crate) use_id: UseId,
    pub(crate) kind: UseKind,
    pub(crate) dirty_queue: Weak<RefCell<BTreeSet<AllocationKey>>>,
}

#[allow(dead_code)]
impl AllocationLease {
    pub(crate) fn new(
        entry: Rc<AllocationEntry>,
        use_id: UseId,
        kind: UseKind,
        dirty_queue: Weak<RefCell<BTreeSet<AllocationKey>>>,
    ) -> Self {
        Self {
            entry,
            use_id,
            kind,
            dirty_queue,
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
        if let Some(dirty) = self.dirty_queue.upgrade() {
            dirty.borrow_mut().insert(self.entry.key());
        }
    }
}
