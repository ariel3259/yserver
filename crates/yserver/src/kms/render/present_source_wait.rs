//! Asynchronous producer waits for Present sources: dma-buf implicit fences
//! for `PresentPixmap`, and explicit DRM syncobj timeline points for
//! `PresentPixmapSynced`.

use std::{
    os::fd::{AsFd, OwnedFd},
    sync::Arc,
};

use nix::poll::{PollFd, PollFlags, PollTimeout, poll};

use crate::kms::render::store::DrawableId;

/// One imported source held alive until its producer fence/timeline point is
/// ready and the core records the deferred copy.
pub(crate) struct PendingPresentSourceWait {
    pub(crate) fd: Option<OwnedFd>,
    pub(crate) source_id: DrawableId,
    /// Keeps an explicitly imported acquire syncobj alive until its timeline
    /// point signals. Implicit dma-buf waits leave this empty.
    pub(crate) syncobj_pin: Option<Arc<super::owned_semaphore::OwnedSemaphore>>,
    /// Used only when DRM_SYNCOBJ_EVENTFD is unavailable and readiness must
    /// be checked through the imported Vulkan timeline semaphore.
    pub(crate) timeline_value: Option<u64>,
    /// False when registration in the stable completion poller failed. Such
    /// entries are still checked from the backend's 1 ms polling fallback.
    pub(crate) registered: bool,
    pub(crate) ready_reported: bool,
}

impl PendingPresentSourceWait {
    pub(crate) fn is_ready(&self) -> bool {
        let Some(fd) = self.fd.as_ref() else {
            let (Some(syncobj), Some(target)) = (&self.syncobj_pin, self.timeline_value) else {
                return true;
            };
            return match syncobj.timeline_value() {
                Ok(current) => current >= target,
                Err(e) => {
                    log::warn!(
                        "deferred Present acquire: vkGetSemaphoreCounterValue failed: {e:?}; \
                         treating as ready"
                    );
                    true
                }
            };
        };
        let mut fds = [PollFd::new(fd.as_fd(), PollFlags::POLLIN)];
        match poll(&mut fds, PollTimeout::ZERO) {
            Ok(0) => false,
            Ok(_) => fds[0].revents().is_some_and(|flags| {
                flags.intersects(PollFlags::POLLIN | PollFlags::POLLERR | PollFlags::POLLHUP)
            }),
            Err(e) => {
                log::warn!("deferred Present source: poll(sync_file): {e}; treating as ready");
                true
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::os::fd::OwnedFd;

    use nix::{
        sys::eventfd::{EfdFlags, EventFd},
        unistd::write,
    };

    use super::*;

    #[test]
    fn readiness_tracks_the_sync_file_signal() {
        let event =
            EventFd::from_value_and_flags(0, EfdFlags::EFD_NONBLOCK | EfdFlags::EFD_CLOEXEC)
                .expect("eventfd");
        let fd: OwnedFd = event.into();
        let wait = PendingPresentSourceWait {
            fd: Some(fd),
            source_id: DrawableId::for_tests(1),
            syncobj_pin: None,
            timeline_value: None,
            registered: false,
            ready_reported: false,
        };

        assert!(!wait.is_ready());
        write(wait.fd.as_ref().unwrap(), &1_u64.to_ne_bytes()).expect("signal eventfd");
        assert!(wait.is_ready());
    }
}
