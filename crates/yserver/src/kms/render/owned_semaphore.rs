//! RAII wrapper for a `vk::Semaphore` so it can be `Arc`-shared
//! for the deferred PRESENT completion path. Destruction happens
//! on the last Arc drop (via `vkDestroySemaphore`), independent of
//! the X11 resource id's lifetime.

use std::sync::Arc;

use ash::vk;

use crate::kms::vk::device::VkContext;

pub(crate) struct OwnedSemaphore {
    vk: Arc<VkContext>,
    semaphore: vk::Semaphore,
    drm_syncobj: Option<(Arc<crate::drm::Device>, ::drm::control::syncobj::Handle)>,
}

impl OwnedSemaphore {
    pub(crate) fn new(vk: Arc<VkContext>, semaphore: vk::Semaphore) -> Self {
        Self {
            vk,
            semaphore,
            drm_syncobj: None,
        }
    }

    pub(crate) fn semaphore(&self) -> vk::Semaphore {
        self.semaphore
    }

    /// Signal a timeline-semaphore value via `vkSignalSemaphore`.
    /// Renamed to `signal_vk` to disambiguate from the
    /// `SyncobjHandle::signal` trait method.
    pub(crate) fn signal_vk(&self, value: u64) -> Result<(), vk::Result> {
        crate::kms::vk::sync::signal_timeline(&self.vk, self.semaphore, value)
    }
}

impl std::fmt::Debug for OwnedSemaphore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OwnedSemaphore")
            .field("semaphore", &self.semaphore)
            .finish_non_exhaustive()
    }
}

impl Drop for OwnedSemaphore {
    fn drop(&mut self) {
        if let Some((drm, handle)) = self.drm_syncobj.take() {
            use ::drm::control::Device as _;
            if let Err(e) = drm.destroy_syncobj(handle) {
                log::warn!("destroy retained DRM syncobj handle failed: {e}");
            }
        }
        if self.semaphore == vk::Semaphore::null() {
            return;
        }
        unsafe {
            self.vk.device.destroy_semaphore(self.semaphore, None);
        }
    }
}

impl yserver_core::backend::SyncobjHandle for OwnedSemaphore {
    fn signal(&self, value: u64) -> std::io::Result<()> {
        self.signal_vk(value)
            .map_err(|e| std::io::Error::other(format!("vkSignalSemaphore: {e:?}")))
    }
}
