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

    pub(crate) fn commit_key(&self) -> super::commit::CommitKey {
        super::commit::CommitKey::new(self.device, self.commit)
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
    pub(crate) sample: Option<crate::kms::owner::clock::ClockSample>,
}

impl PresentDisposition {
    pub(crate) fn new(completion: CompletionDisposition, release: ReleaseDisposition) -> Self {
        Self {
            completion,
            release,
            sample: None,
        }
    }

    pub(crate) fn with_sample(mut self, sample: crate::kms::owner::clock::ClockSample) -> Self {
        self.sample = Some(sample);
        self
    }

    pub(crate) fn pending() -> Self {
        Self {
            completion: CompletionDisposition::Pending,
            release: ReleaseDisposition::Retained,
            sample: None,
        }
    }

    pub(crate) fn emitted_retained() -> Self {
        Self {
            completion: CompletionDisposition::Emitted,
            release: ReleaseDisposition::Retained,
            sample: None,
        }
    }

    pub(crate) fn emitted_released() -> Self {
        Self {
            completion: CompletionDisposition::Emitted,
            release: ReleaseDisposition::Released,
            sample: None,
        }
    }

    pub(crate) fn suppressed() -> Self {
        Self {
            completion: CompletionDisposition::Suppressed,
            release: ReleaseDisposition::Retained,
            sample: None,
        }
    }
}
