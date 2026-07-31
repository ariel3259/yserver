//! Deferred PRESENT completion queue (Stage 5 Task 6.1).
//!
//! Owns per-entry state for the v2 backend's `enqueue_present_completion`
//! and `drain_completed_present_events` trait impls. Internal types
//! never escape the `yserver` crate; the trait surface exchanges
//! the public `CompletedPresentEvent` only.
//!
//! Spec: `docs/superpowers/specs/2026-05-23-deferred-present-completion-design.md`.

use std::{
    os::fd::{AsFd, AsRawFd, BorrowedFd, OwnedFd},
    sync::{Arc, OnceLock},
};

use yserver_core::backend::{CompletedPresentEvent, SyncobjHandle, XshmfenceHandle};

use crate::kms::render::platform::{FenceTicket, PresentCompletionSignal};

/// One deferred PRESENT completion payload. The drain fires the
/// wake signal via `wake_pin` + returns the `event` payload to the
/// main loop.
#[derive(Debug)]
pub(crate) struct PendingPresentEntry {
    /// Backend lookup key for trace correlation. This is not the client XID
    /// carried by `event.host_xid`.
    pub(crate) source_host_xid: u32,
    /// Client-owned Present source. Kept alive until submission so the
    /// completed copy can be published as a shared READ fence.
    pub(crate) source_dmabuf: Option<OwnedFd>,
    /// Lifetime pin on the underlying wake primitive. Survives a
    /// mid-flight `XFixesDestroyFence` / `FreeSyncobj`.
    pub(crate) wake_pin: PinnedWake,
    /// Public-facing event payload, returned by `drain_*` to the
    /// main loop.
    pub(crate) event: CompletedPresentEvent,
}

/// Publish the copy-completion sync_file to every implicit-sync Present
/// source in the batch. The kernel duplicates `sync_fd`, so the same exported
/// fence remains available for event-loop completion polling.
pub(crate) fn publish_source_read_fences(entries: &[PendingPresentEntry], sync_fd: BorrowedFd<'_>) {
    static SYNC_TRACE: OnceLock<bool> = OnceLock::new();
    let trace =
        *SYNC_TRACE.get_or_init(|| std::env::var_os("YSERVER_PRESENT_SYNC_TRACE").is_some());

    for entry in entries {
        let Some(dmabuf) = entry.source_dmabuf.as_ref() else {
            continue;
        };
        match crate::kms::vk::dri3::import_dmabuf_read_fence(dmabuf.as_fd(), sync_fd) {
            Ok(()) => {
                if trace {
                    let writer = current_writer_state(dmabuf.as_fd());
                    log::info!(
                        target: "present_sync_trace",
                        "PRESENT-SYNC serial={} src=0x{:x} read_fence=published post_publish_writer={writer}",
                        entry.event.serial,
                        entry.source_host_xid,
                    );
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::Unsupported => {
                if trace {
                    log::info!(
                        target: "present_sync_trace",
                        "PRESENT-SYNC serial={} src=0x{:x} read_fence=unsupported",
                        entry.event.serial,
                        entry.source_host_xid,
                    );
                }
            }
            Err(error) => {
                log::warn!(
                    "PRESENT: failed to publish source dma-buf READ fence for serial {}: {error}",
                    entry.event.serial
                );
            }
        }
    }
}

fn current_writer_state(dmabuf: BorrowedFd<'_>) -> &'static str {
    use crate::kms::vk::dri3::{ExportedSyncFile, export_dmabuf_read_access_sync_file};

    let sync_fd = match export_dmabuf_read_access_sync_file(dmabuf) {
        ExportedSyncFile::Idle => return "idle",
        ExportedSyncFile::Unsupported => return "unsupported",
        ExportedSyncFile::Fd(fd) => fd,
    };
    let mut pollfd = libc::pollfd {
        fd: sync_fd.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    // SAFETY: `pollfd` contains one live sync-file fd and timeout zero never
    // blocks the server loop.
    match unsafe { libc::poll(std::ptr::addr_of_mut!(pollfd), 1, 0) } {
        0 => "pending",
        n if n > 0 && (pollfd.revents & libc::POLLIN) != 0 => "signaled",
        _ => "poll-error",
    }
}

/// Readiness primitive for a submitted batch of PRESENT completions.
pub(crate) enum PresentBatchWait {
    /// Linux sync_file fd exported from a dedicated completion
    /// semaphore. This is the hot path.
    Fd(OwnedFd),
    /// Export returned `-1`, meaning already signaled.
    Ready,
    /// Degraded path if fd export fails. Polls `ticket` through
    /// `Backend::next_wakeup`, but should not occur on normal Linux
    /// Vulkan stacks.
    Poll,
}

/// Submitted-but-not-yet-emitted PRESENT completion batch.
pub(crate) struct PendingPresentBatch {
    pub(crate) wait: PresentBatchWait,
    /// Optional internal fence for degraded polling only. The hot fd
    /// path does not need this for readiness.
    pub(crate) ticket: Option<FenceTicket>,
    /// Keeps the dedicated export-only semaphore alive until the
    /// exported sync_file has fired.
    pub(crate) signal: Option<PresentCompletionSignal>,
    pub(crate) events: Vec<PendingPresentEntry>,
}

/// Wake-target lifetime pin variants. The drain dispatches signal
/// via the held `Arc` regardless of whether the X11 resource id is
/// still in the registry.
#[derive(Debug)]
pub(crate) enum PinnedWake {
    Pixmap(Arc<dyn XshmfenceHandle>),
    PixmapSynced {
        handle: Arc<dyn SyncobjHandle>,
        value: u64,
    },
    /// Client passed no wake object (idle_fence_xid == 0 or
    /// release_syncobj == 0). Drain skips the signal step; X11 event
    /// emission still happens.
    None,
}

#[cfg(test)]
mod tests {
    use super::*;
    use yserver_core::backend::PresentWake;
    use yserver_protocol::x11::ClientId;

    /// Smoke test that the types compile + can be constructed.
    /// Real semantics tested in `KmsBackend` integration tests.
    #[test]
    fn pinned_wake_none_constructs() {
        let pin = PinnedWake::None;
        match pin {
            PinnedWake::None => {}
            _ => panic!("unexpected variant"),
        }
    }

    #[test]
    fn completed_present_event_carries_payload() {
        let event = CompletedPresentEvent {
            client_id: ClientId(7),
            serial: 42,
            host_xid: 0x100001,
            dst_host_xid: 0xE00001,
            options: 0,
            present_id: 0,
            wake: PresentWake::Pixmap {
                idle_fence_xid: 0xCC,
            },
        };
        assert_eq!(event.serial, 42);
    }
}
