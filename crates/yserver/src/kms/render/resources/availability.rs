use std::{cell::RefCell, collections::BTreeMap};

use crate::{kms::owner::identity::IncarnationId, platform::drm::DrmDeviceKey};

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct AllocationKey {
    pub device: DrmDeviceKey,
    pub incarnation: IncarnationId,
    pub generation: u64,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct UseId(pub(crate) u64);

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct ObligationId(pub(crate) u64);

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UseKind {
    Retain,
    Read,
    Write,
    Kms,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ObligationKind {
    KmsRelease,
    Gpu,
    Read,
    ForeignReturn,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ResourceError {
    Busy,
    WrongIncarnation,
    Exhausted,
    Frozen,
    Detached,
    InvalidProof,
    InvalidState,
}

impl std::fmt::Display for ResourceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for ResourceError {}

#[allow(dead_code)]
#[derive(Debug)]
pub(crate) struct EntryAvailability {
    pub(crate) frozen: bool,
    pub(crate) live_uses: BTreeMap<UseId, UseKind>,
    pub(crate) pending_obligations: BTreeMap<ObligationId, ObligationKind>,
    pub(crate) kms_dispositions: BTreeMap<
        ObligationId,
        (
            crate::kms::render::resources::commit::GroupMember,
            crate::kms::owner::identity::CommitId,
            crate::kms::render::resources::handoff::KmsDisposition,
        ),
    >,
}

#[allow(dead_code)]
impl EntryAvailability {
    pub(crate) fn new() -> Self {
        Self {
            frozen: false,
            live_uses: BTreeMap::new(),
            pending_obligations: BTreeMap::new(),
            kms_dispositions: BTreeMap::new(),
        }
    }

    pub(crate) fn live_use_count(&self) -> usize {
        self.live_uses.len()
    }

    pub(crate) fn pending_obligation_count(&self) -> usize {
        self.pending_obligations.len()
    }

    pub(crate) fn live_readers(&self) -> usize {
        self.live_uses
            .values()
            .filter(|k| matches!(k, UseKind::Read))
            .count()
    }

    pub(crate) fn live_writers(&self) -> usize {
        self.live_uses
            .values()
            .filter(|k| matches!(k, UseKind::Write))
            .count()
    }

    pub(crate) fn live_kms(&self) -> usize {
        self.live_uses
            .values()
            .filter(|k| matches!(k, UseKind::Kms))
            .count()
    }

    pub(crate) fn is_compatible(&self, usage: UseKind) -> bool {
        match usage {
            UseKind::Retain => true,
            UseKind::Read => self.live_writers() == 0,
            UseKind::Kms => self.live_writers() == 0,
            UseKind::Write => {
                self.live_readers() == 0
                    && self.live_writers() == 0
                    && self.live_kms() == 0
                    && self.pending_obligation_count() == 0
            }
        }
    }
}

#[allow(dead_code)]
pub(crate) struct AllocationEntry {
    pub(crate) key: AllocationKey,
    pub(in crate::kms::render::resources) payload: RefCell<Option<super::AllocationPayload>>,
    pub(crate) availability: RefCell<EntryAvailability>,
}

impl std::fmt::Debug for AllocationEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AllocationEntry")
            .field("key", &self.key)
            .field("availability", &self.availability)
            .finish()
    }
}

#[allow(dead_code)]
impl AllocationEntry {
    pub(crate) fn new(key: AllocationKey, payload: super::AllocationPayload) -> Self {
        Self {
            key,
            payload: RefCell::new(Some(payload)),
            availability: RefCell::new(EntryAvailability::new()),
        }
    }

    pub(crate) fn key(&self) -> AllocationKey {
        self.key
    }

    pub(crate) fn frozen(&self) -> bool {
        self.availability.borrow().frozen
    }

    pub(crate) fn live_use_count(&self) -> usize {
        self.availability.borrow().live_use_count()
    }

    pub(crate) fn pending_obligation_count(&self) -> usize {
        self.availability.borrow().pending_obligation_count()
    }

    pub(crate) fn remove_use(&self, use_id: UseId) -> Option<UseKind> {
        self.availability.borrow_mut().live_uses.remove(&use_id)
    }

    pub(crate) fn take_payload(&self) -> Option<super::AllocationPayload> {
        self.payload.borrow_mut().take()
    }
}

#[allow(dead_code)]
pub(crate) fn can_destroy(entry: &AllocationEntry) -> bool {
    !entry.frozen() && entry.live_use_count() == 0 && entry.pending_obligation_count() == 0
}
