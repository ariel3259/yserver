//! Exact install/restore completion-mechanism qualification.

use std::collections::BTreeSet;

use super::identity::{CommitId, IncarnationId};

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum CompletionQualification {
    Unqualified {
        topology_generation: u64,
    },
    Awaiting {
        topology_generation: u64,
        commit: CommitId,
    },
    Qualified {
        topology_generation: u64,
        commit: CommitId,
    },
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct CompletionCaps {
    pub(crate) incarnation: IncarnationId,
    pub(crate) topology_generation: u64,
    pub(crate) atomic_enabled: bool,
    pub(crate) crtc_in_event: bool,
    pub(crate) monotonic: bool,
    pub(crate) out_fence_crtcs: BTreeSet<u32>,
}

impl CompletionCaps {
    pub(in crate::kms) fn new(
        incarnation: IncarnationId,
        topology_generation: u64,
        atomic_enabled: bool,
        crtc_in_event: bool,
        monotonic: bool,
        out_fence_crtcs: BTreeSet<u32>,
    ) -> Self {
        Self {
            incarnation,
            topology_generation,
            atomic_enabled,
            crtc_in_event,
            monotonic,
            out_fence_crtcs,
        }
    }

    #[doc(hidden)]
    pub fn new_for_tests(
        incarnation: IncarnationId,
        topology_generation: u64,
        atomic_enabled: bool,
        crtc_in_event: bool,
        monotonic: bool,
        out_fence_crtcs: BTreeSet<u32>,
    ) -> Self {
        Self::new(
            incarnation,
            topology_generation,
            atomic_enabled,
            crtc_in_event,
            monotonic,
            out_fence_crtcs,
        )
    }

    pub fn incarnation(&self) -> IncarnationId {
        self.incarnation
    }

    pub fn topology_generation(&self) -> u64 {
        self.topology_generation
    }

    pub fn atomic_enabled(&self) -> bool {
        self.atomic_enabled
    }

    pub fn crtc_in_event(&self) -> bool {
        self.crtc_in_event
    }

    pub fn monotonic(&self) -> bool {
        self.monotonic
    }

    pub fn out_fence_crtcs(&self) -> &BTreeSet<u32> {
        &self.out_fence_crtcs
    }

    pub fn is_structurally_capable(&self) -> bool {
        self.atomic_enabled && self.crtc_in_event && self.monotonic
    }
}
