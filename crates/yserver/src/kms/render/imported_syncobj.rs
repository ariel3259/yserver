//! A DRI3 1.4 syncobj imported from a client fd, held as a process-local
//! DRM syncobj handle.
//!
//! This deliberately has no Vulkan in it. A DRM syncobj is a kernel object
//! and every operation the server needs — signal, query, eventfd — has a DRM
//! ioctl. Importing it into a `VkSemaphore` instead only works where the
//! driver's `OPAQUE_FD` payload happens to be a DRM syncobj, which is true on
//! Mesa and false on NVIDIA proprietary
//! (`vkImportSemaphoreFdKHR` → `VK_ERROR_INITIALIZATION_FAILED`). See
//! docs/superpowers/specs/2026-08-08-dri3-syncobj-drm-signal-design.md.
//!
//! The sibling `OwnedSemaphore` keeps the Vulkan path for XSync `Fence`
//! resources, which need a real `VkSemaphore` for `FDFromFence`'s sync_file
//! export.
//!
//! The `Arc<crate::drm::Device>` here MUST be the render node — the device
//! DRI3 hands the client (`PlatformBackend::render_node_device`), never the
//! KMS node. See the spec's "Which fd to ask" section.

use std::{
    os::fd::{AsFd, BorrowedFd, OwnedFd},
    sync::Arc,
};

use ::drm::control::{Device as DrmControlDevice, syncobj};

pub(crate) struct ImportedSyncobj {
    drm: Arc<crate::drm::Device>,
    handle: syncobj::Handle,
}

impl ImportedSyncobj {
    /// Import a client's `DRM_SYNCOBJ` fd as a process-local handle. The fd is
    /// only borrowed — `DRM_IOCTL_SYNCOBJ_FD_TO_HANDLE` does not consume it —
    /// so the caller keeps ownership and drops it normally. Importing a
    /// syncobj fd creates a NEW handle (with its own reference) for every
    /// import; the underlying `struct drm_syncobj` is shared, which is what
    /// lets a server-side signal reach the client's handle.
    pub(crate) fn import(
        drm: Arc<crate::drm::Device>,
        fd: BorrowedFd<'_>,
    ) -> std::io::Result<Self> {
        let handle = drm.fd_to_syncobj(fd, false)?;
        Ok(Self { drm, handle })
    }

    /// Current timeline value. Replaces `vkGetSemaphoreCounterValue` in the
    /// deferred-acquire polling fallback.
    pub(crate) fn timeline_value(&self) -> std::io::Result<u64> {
        let mut points = [0u64; 1];
        self.drm
            .syncobj_timeline_query(&[self.handle], &mut points, false)?;
        Ok(points[0])
    }

    /// Register a non-blocking kernel notification for a timeline point.
    /// Unchanged in behaviour from the previous `OwnedSemaphore` version —
    /// that method already went through DRM.
    pub(crate) fn signaled_eventfd(&self, value: u64) -> std::io::Result<OwnedFd> {
        use nix::sys::eventfd::{EfdFlags, EventFd};

        let event =
            EventFd::from_value_and_flags(0, EfdFlags::EFD_NONBLOCK | EfdFlags::EFD_CLOEXEC)
                .map_err(|e| std::io::Error::other(format!("eventfd: {e}")))?;
        self.drm
            .syncobj_eventfd(self.handle, value, event.as_fd(), false)?;
        Ok(event.into())
    }
}

impl std::fmt::Debug for ImportedSyncobj {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ImportedSyncobj")
            .field("handle", &self.handle)
            .finish_non_exhaustive()
    }
}

impl Drop for ImportedSyncobj {
    fn drop(&mut self) {
        if let Err(e) = self.drm.destroy_syncobj(self.handle) {
            log::warn!("destroy imported DRM syncobj handle failed: {e}");
        }
    }
}

impl yserver_core::backend::SyncobjHandle for ImportedSyncobj {
    /// Host-signal a timeline point. Replaces `vkSignalSemaphore`, which was
    /// also a host operation.
    ///
    /// Note the kernel CLAMPS: signalling a point at or below the current
    /// value succeeds silently and leaves the timeline where it was. Callers
    /// cannot use the return value to detect an out-of-order release.
    fn signal(&self, value: u64) -> std::io::Result<()> {
        self.drm.syncobj_timeline_signal(&[self.handle], &[value])
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::{os::fd::AsFd, sync::Arc};

    use ::drm::control::Device as DrmControlDevice;

    use super::*;

    /// Open a render node, or skip. Every test here needs real DRM ioctls;
    /// they are `#[ignore]` so CI never runs them, but a machine without a
    /// node should skip rather than fail.
    ///
    /// Do NOT hardcode `/dev/dri/renderD128`. `kms/render_node.rs:1-8` states
    /// the rule outright — "we deliberately do **not** hardcode
    /// `/dev/dri/renderD128` — on multi-GPU hosts that selects the wrong
    /// device" — and the nvidia box became exactly such a host on 2026-08-08:
    /// `renderD128` is nvidia-drm and `renderD129` is the Raphael iGPU. A
    /// hardcoded 128 would make a run intended to validate Mesa silently
    /// exercise nvidia-drm and report green.
    ///
    /// Honour `YSERVER_TEST_RENDER_NODE` so a Mesa run can be directed at the
    /// amdgpu node, and enumerate otherwise rather than guessing.
    pub(crate) fn render_node() -> Option<Arc<crate::drm::Device>> {
        if let Ok(path) = std::env::var("YSERVER_TEST_RENDER_NODE") {
            return crate::drm::Device::open_render_node(&path)
                .ok()
                .map(Arc::new);
        }
        let mut paths: Vec<_> = std::fs::read_dir("/dev/dri")
            .ok()?
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with("renderD"))
            })
            .collect();
        paths.sort();
        paths
            .iter()
            .find_map(|p| crate::drm::Device::open_render_node(p.to_str()?).ok())
            .map(Arc::new)
    }

    /// Full round trip mirroring the server's sequence: the client exports a
    /// syncobj fd, the server imports it, signals a release point, and the
    /// client's own handle observes it through its own separate handle.
    /// Run with `cargo test -p yserver --lib imported_syncobj -- --ignored`.
    #[test]
    #[ignore = "needs a DRM render node"]
    fn signal_reaches_the_clients_handle() {
        let Some(drm) = render_node() else {
            eprintln!("skipping: no render node");
            return;
        };

        let client_handle = drm.create_syncobj(false).expect("create syncobj");
        let fd = drm.syncobj_to_fd(client_handle, false).expect("export fd");

        let imported = ImportedSyncobj::import(drm.clone(), fd.as_fd()).expect("import");
        assert_eq!(imported.timeline_value().expect("query"), 0);

        yserver_core::backend::SyncobjHandle::signal(&imported, 7).expect("signal");

        // The client must observe the release through ITS handle, not the
        // server's, or the two are not aliasing one payload and the client
        // would wait forever.
        let mut points = [0u64; 1];
        drm.syncobj_timeline_query(&[client_handle], &mut points, false)
            .expect("client query");
        assert_eq!(
            points[0], 7,
            "server signal did not reach the client handle"
        );

        drm.destroy_syncobj(client_handle).expect("destroy");
    }

    /// The acquire path's kernel notification.
    #[test]
    #[ignore = "needs a DRM render node"]
    fn eventfd_fires_on_the_registered_point() {
        let Some(drm) = render_node() else {
            eprintln!("skipping: no render node");
            return;
        };
        let client_handle = drm.create_syncobj(false).expect("create syncobj");
        let fd = drm.syncobj_to_fd(client_handle, false).expect("export fd");
        let imported = ImportedSyncobj::import(drm.clone(), fd.as_fd()).expect("import");

        let event = imported.signaled_eventfd(9).expect("register eventfd");
        let mut buf = [0u8; 8];
        assert!(
            nix::unistd::read(event.as_fd(), &mut buf).is_err(),
            "eventfd readable before the point was signalled",
        );

        yserver_core::backend::SyncobjHandle::signal(&imported, 9).expect("signal");
        std::thread::sleep(std::time::Duration::from_millis(50));
        assert!(
            nix::unistd::read(event.as_fd(), &mut buf).is_ok(),
            "eventfd never fired after the point was signalled",
        );

        drm.destroy_syncobj(client_handle).expect("destroy");
    }

    /// Documents measured kernel behaviour the spec depends on: a stale or
    /// duplicate timeline point is CLAMPED and returns success, it is not
    /// rejected. Release replay after teardown therefore cannot be detected
    /// by checking the signal's return value.
    #[test]
    #[ignore = "needs a DRM render node"]
    fn a_stale_point_is_clamped_not_rejected() {
        use yserver_core::backend::SyncobjHandle as _;

        let Some(drm) = render_node() else {
            eprintln!("skipping: no render node");
            return;
        };
        let client_handle = drm.create_syncobj(false).expect("create syncobj");
        let fd = drm.syncobj_to_fd(client_handle, false).expect("export fd");
        let imported = ImportedSyncobj::import(drm.clone(), fd.as_fd()).expect("import");

        imported.signal(10).expect("signal 10");
        imported
            .signal(5)
            .expect("a stale point must still return Ok");
        assert_eq!(
            imported.timeline_value().expect("query"),
            10,
            "the kernel must clamp to the max, not regress the timeline",
        );

        drm.destroy_syncobj(client_handle).expect("destroy");
    }
}
