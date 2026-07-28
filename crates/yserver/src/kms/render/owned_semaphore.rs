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

    pub(crate) fn new_drm_syncobj(
        vk: Arc<VkContext>,
        semaphore: vk::Semaphore,
        drm: Arc<crate::drm::Device>,
        handle: ::drm::control::syncobj::Handle,
    ) -> Self {
        Self {
            vk,
            semaphore,
            drm_syncobj: Some((drm, handle)),
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

    pub(crate) fn timeline_value(&self) -> Result<u64, vk::Result> {
        unsafe { self.vk.device.get_semaphore_counter_value(self.semaphore) }
    }

    /// Register a non-blocking kernel notification for a timeline point.
    /// The retained DRM handle aliases the same imported syncobj payload as
    /// the Vulkan semaphore.
    pub(crate) fn signaled_eventfd(&self, value: u64) -> std::io::Result<std::os::fd::OwnedFd> {
        use std::os::fd::AsFd;

        use ::drm::control::Device as _;
        use nix::sys::eventfd::{EfdFlags, EventFd};

        let Some((drm, handle)) = &self.drm_syncobj else {
            return Err(std::io::Error::other(
                "timeline semaphore has no retained DRM syncobj handle",
            ));
        };
        let event =
            EventFd::from_value_and_flags(0, EfdFlags::EFD_NONBLOCK | EfdFlags::EFD_CLOEXEC)
                .map_err(|e| std::io::Error::other(format!("eventfd: {e}")))?;
        drm.syncobj_eventfd(*handle, value, event.as_fd(), false)?;
        Ok(event.into())
    }

    /// Test-only constructor: holds a null semaphore handle. Drop
    /// is a no-op (vkDestroySemaphore on null is allowed by Vulkan
    /// spec but the guard below skips it anyway).
    #[cfg(test)]
    pub(crate) fn for_tests_dummy(vk: Arc<VkContext>) -> Self {
        Self {
            vk,
            semaphore: vk::Semaphore::null(),
            drm_syncobj: None,
        }
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
