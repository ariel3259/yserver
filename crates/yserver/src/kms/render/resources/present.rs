#![allow(dead_code)]

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PresentKey {
    pub(crate) device: crate::platform::drm::DrmDeviceKey,
    pub(crate) incarnation: crate::kms::owner::identity::IncarnationId,
    pub(crate) commit: crate::kms::owner::identity::CommitId,
    pub(crate) present_id: u64,
}

impl PresentKey {
    pub(crate) fn new(
        device: crate::platform::drm::DrmDeviceKey,
        incarnation: crate::kms::owner::identity::IncarnationId,
        commit: crate::kms::owner::identity::CommitId,
        present_id: u64,
    ) -> Self {
        Self {
            device,
            incarnation,
            commit,
            present_id,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompletionDisposition {
    Pending,
    Emitted,
    Suppressed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReleaseDisposition {
    Retained,
    Released,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PresentDisposition {
    pub(crate) completion: CompletionDisposition,
    pub(crate) release: ReleaseDisposition,
}

impl PresentDisposition {
    pub(crate) fn new(completion: CompletionDisposition, release: ReleaseDisposition) -> Self {
        Self {
            completion,
            release,
        }
    }

    pub(crate) fn pending() -> Self {
        Self {
            completion: CompletionDisposition::Pending,
            release: ReleaseDisposition::Retained,
        }
    }

    pub(crate) fn emitted_retained() -> Self {
        Self {
            completion: CompletionDisposition::Emitted,
            release: ReleaseDisposition::Retained,
        }
    }

    pub(crate) fn emitted_released() -> Self {
        Self {
            completion: CompletionDisposition::Emitted,
            release: ReleaseDisposition::Released,
        }
    }

    pub(crate) fn suppressed() -> Self {
        Self {
            completion: CompletionDisposition::Suppressed,
            release: ReleaseDisposition::Retained,
        }
    }
}
