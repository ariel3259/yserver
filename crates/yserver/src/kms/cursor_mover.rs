//! Off-thread cursor mover — the blocking legacy `drmModeMoveCursor`
//! ioctl leaves the core thread.
//!
//! Why: on nvidia-drm the legacy cursor move becomes a *blocking* atomic
//! commit that waits for the pending page flip, ~11.5 ms mean / 16.3 ms max
//! with `ebusy=0` (measured, GTX-1050, 1200 moves in one drag). That stalled
//! the single-threaded loop on every drag, which is why the HW cursor was
//! disabled on nvidia — and that in turn keeps direct scanout off, since
//! `scanout_direct_eligible` has `cursor_hw` as a hard conjunct.
//!
//! The ioctl does not get faster here. What changes is who waits: the cursor
//! moves from "tied to compose cadence" to "tied to refresh, independent of
//! the render loop". On nvidia it stays vblank-paced — that is the goal, not
//! a shortfall.
//!
//! What makes this cheap: `move_cursor` needs only `&Device`, and `Device` is
//! `{ File, String }` — `Send + Sync` by construction — so a cloned
//! `Arc<Device>` crosses threads with no `unsafe` and no wrapper. The worker
//! never touches `CursorPlane` (whose mmap pointer is `Send` but not `Sync`);
//! it needs only a target position and the CRTCs the plane is bound on.
//!
//! Design doc: `docs/superpowers/specs/2026-08-15-nvidia-hw-cursor-worker-design.md`.

use std::sync::{
    Arc, Condvar, Mutex,
    atomic::{AtomicU32, AtomicU64, Ordering},
};

use ::drm::control::{Device as ControlDevice, crtc};

use crate::drm::Device;

/// A cursor position to apply, in root space, with the sprite hotspot that
/// was current when it was posted.
///
/// `seq` is allocated on the core thread only, so sequence order is core
/// program order — that is what makes the `> last_applied` check a correct
/// ordering test rather than a heuristic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CursorTarget {
    pub(crate) seq: u64,
    pub(crate) x: i32,
    pub(crate) y: i32,
    pub(crate) hot_x: u16,
    pub(crate) hot_y: u16,
}

/// The CRTCs the cursor plane is currently bound on, with their layout
/// origins. Republished by the core thread on show / hide / rebind — rare
/// events, never the hot path.
///
/// `generation` exists so a stale snapshot is detectable rather than merely
/// improbable: a worker must not resurrect a binding the core thread has
/// just torn down.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct CursorCrtcSnapshot {
    pub(crate) generation: u64,
    pub(crate) crtcs: Vec<(crtc::Handle, i32, i32)>,
}

/// Root-space cursor position → CRTC-local image-top-left, hotspot removed.
///
/// Moved here from `render/platform.rs` so the worker and the core thread
/// share one implementation: moving the ioctl to a thread must not also be a
/// change in geometry.
pub(crate) fn cursor_root_to_crtc_local(
    x: i32,
    y: i32,
    layout_x: i32,
    layout_y: i32,
    hot_x: u16,
    hot_y: u16,
) -> (i32, i32) {
    (
        x - layout_x - i32::from(hot_x),
        y - layout_y - i32::from(hot_y),
    )
}

/// Per-CRTC `move_cursor` arguments for `target` against `snapshot`.
/// The snapshot holds only CRTCs the plane is bound on, so no visibility
/// filter is needed here; the kernel clips off-output coordinates itself.
pub(crate) fn crtc_targets(
    snapshot: &CursorCrtcSnapshot,
    target: &CursorTarget,
) -> Vec<(crtc::Handle, i32, i32)> {
    snapshot
        .crtcs
        .iter()
        .map(|&(crtc, layout_x, layout_y)| {
            let (cx, cy) = cursor_root_to_crtc_local(
                target.x,
                target.y,
                layout_x,
                layout_y,
                target.hot_x,
                target.hot_y,
            );
            (crtc, cx, cy)
        })
        .collect()
}

/// Take the mailbox's target if it is newer than what was last applied.
///
/// Latest-wins is structural (one slot: a post overwrites, it does not
/// queue). The `seq > last_applied` test is the separate ordering
/// invariant: a target taken before a core-thread `show` must be dropped
/// rather than applied on top of the position that `show` established, or
/// the cursor jumps back and stays there until the next motion event.
pub(crate) fn take_applicable(
    slot: &mut Option<CursorTarget>,
    last_applied: u64,
) -> Option<CursorTarget> {
    let target = slot.take()?;
    (target.seq > last_applied).then_some(target)
}

/// Whether a snapshot held at `held_generation` has been superseded.
pub(crate) fn snapshot_is_stale(held_generation: u64, published_generation: u64) -> bool {
    held_generation < published_generation
}

/// Mailbox contents. `shutdown` lives inside the same mutex as the slot so
/// the condvar wait has exactly one predicate to re-check.
struct MailboxState {
    slot: Option<CursorTarget>,
    shutdown: bool,
}

/// State shared between the core thread and the worker.
///
/// Lock order is **`gate` → `snapshot`**, and the mailbox lock is never held
/// across either: the worker takes its target, releases the mailbox, then
/// takes the gate. Violating that order is the only way to deadlock this.
struct MoverShared {
    device: Arc<Device>,
    mailbox: Mutex<MailboxState>,
    ready: Condvar,
    /// The ioctl gate. Held across every cursor ioctl — the worker's
    /// `move_cursor` and the core thread's `set_cursor2` / `move_cursor`
    /// pair alike — and carries the highest sequence applied by anyone.
    ///
    /// The sequence check has to happen *under* this lock: checking it
    /// before the ioctl and then blocking ~1 vblank inside the ioctl is
    /// exactly the window in which a core-thread `show` would be overwritten
    /// by a stale position.
    gate: Mutex<u64>,
    snapshot: Mutex<Arc<CursorCrtcSnapshot>>,
    /// Drained by the core thread into the existing `cursor_move_ebusy`
    /// telemetry, so that counter keeps meaning what it means today.
    ebusy: AtomicU32,
}

/// Poison recovery: a panic elsewhere must not wedge the cursor.
fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl MoverShared {
    /// Apply the taken target: re-check the ordering invariant **under the
    /// gate**, re-read the snapshot, then block in the ioctl per bound CRTC.
    ///
    /// The invariant is re-checked here rather than at take time on purpose:
    /// checking it before the gate and then blocking ~1 vblank inside the
    /// ioctl is exactly the window a core-thread `show` slips through.
    fn apply(&self, pending: &mut Option<CursorTarget>) {
        let mut last_applied = lock(&self.gate);
        let Some(target) = take_applicable(pending, *last_applied) else {
            return;
        };
        let snapshot = Arc::clone(&*lock(&self.snapshot));
        for (crtc, cx, cy) in crtc_targets(&snapshot, &target) {
            #[allow(deprecated)]
            let result = self.device.move_cursor(crtc, (cx, cy));
            if let Err(e) = result {
                if e.raw_os_error() == Some(libc::EBUSY) {
                    self.ebusy.fetch_add(1, Ordering::Relaxed);
                } else {
                    log::debug!("cursor mover: move on {crtc:?} failed: {e}");
                }
            }
        }
        *last_applied = target.seq;
    }
}

/// Free pacing: block in `move_cursor`, return, take the newest target,
/// block again. The kernel holds the worker for exactly one flip, which is
/// the correct cadence and needs no coordination to achieve.
fn worker_loop(shared: &MoverShared) {
    loop {
        // Take the slot under the mailbox lock and RELEASE it before the
        // gate — lock order is gate → snapshot, and the gate must never be
        // taken while the mailbox is held.
        let mut pending = {
            let mut state = lock(&shared.mailbox);
            loop {
                if state.shutdown {
                    return;
                }
                if state.slot.is_some() {
                    break state.slot.take();
                }
                state = shared.ready.wait(state).unwrap_or_else(|e| e.into_inner());
            }
        };
        shared.apply(&mut pending);
    }
}

/// Owns the mover thread. Dropping it shuts the worker down and joins it.
pub(crate) struct CursorMover {
    shared: Arc<MoverShared>,
    join: Option<std::thread::JoinHandle<()>>,
    next_seq: AtomicU64,
    next_generation: AtomicU64,
}

impl CursorMover {
    /// Spawn the worker against a cloned `Arc<Device>`. The initial snapshot
    /// is empty: the plane is bound on nothing until the scene shows it.
    pub(crate) fn spawn(device: Arc<Device>) -> Self {
        let shared = Arc::new(MoverShared {
            device,
            mailbox: Mutex::new(MailboxState {
                slot: None,
                shutdown: false,
            }),
            ready: Condvar::new(),
            gate: Mutex::new(0),
            snapshot: Mutex::new(Arc::new(CursorCrtcSnapshot::default())),
            ebusy: AtomicU32::new(0),
        });
        let worker_shared = Arc::clone(&shared);
        let join = std::thread::Builder::new()
            .name("cursor-mover".to_string())
            .spawn(move || worker_loop(&worker_shared))
            .ok();
        if join.is_none() {
            log::warn!("cursor mover: thread spawn failed; cursor moves will be dropped");
        }
        Self {
            shared,
            join,
            next_seq: AtomicU64::new(0),
            next_generation: AtomicU64::new(0),
        }
    }

    /// Pointer fast path. Overwrites the mailbox and notifies — no ioctl, no
    /// blocking, no `&mut CursorPlane` borrow. Intermediate positions are
    /// discarded by construction.
    pub(crate) fn post(&self, x: i32, y: i32, hot_x: u16, hot_y: u16) {
        let seq = self.next_seq.fetch_add(1, Ordering::Relaxed) + 1;
        {
            let mut state = lock(&self.shared.mailbox);
            state.slot = Some(CursorTarget {
                seq,
                x,
                y,
                hot_x,
                hot_y,
            });
        }
        self.shared.ready.notify_one();
    }

    /// Run `f` with every cursor ioctl serialized against the worker, then
    /// record the position `f` established as the newest applied one.
    ///
    /// This is what `show` / `hide` / rebind go through. They already issue
    /// two blocking ioctls from the core thread; waiting up to one worker
    /// ioctl for the gate is a bounded addition to an already-blocking rare
    /// path. The hot path never takes this lock.
    pub(crate) fn with_ioctl_gate<R>(&self, f: impl FnOnce() -> R) -> R {
        let seq = self.next_seq.fetch_add(1, Ordering::Relaxed) + 1;
        let mut last_applied = lock(&self.shared.gate);
        let out = f();
        // `max` rather than assignment: sequences are allocated on the core
        // thread in program order, so this cannot regress today — but a
        // future poster on another thread would silently un-apply a newer
        // position, and that is not a failure worth leaving discoverable
        // only in the field.
        *last_applied = (*last_applied).max(seq);
        out
    }

    /// Publish the CRTCs the plane is bound on. Call from **inside**
    /// [`Self::with_ioctl_gate`], so the snapshot and the ioctl that changed
    /// the binding become visible to the worker together.
    pub(crate) fn publish_snapshot(&self, crtcs: Vec<(crtc::Handle, i32, i32)>) {
        let generation = self.next_generation.fetch_add(1, Ordering::Relaxed) + 1;
        let mut slot = lock(&self.shared.snapshot);
        debug_assert!(
            !snapshot_is_stale(generation, slot.generation),
            "snapshot generations must be published in order"
        );
        *slot = Arc::new(CursorCrtcSnapshot { generation, crtcs });
    }

    /// Drain the worker's EBUSY count into the caller's telemetry.
    pub(crate) fn take_ebusy(&self) -> u32 {
        self.shared.ebusy.swap(0, Ordering::Relaxed)
    }

    #[cfg(test)]
    pub(crate) fn applied_seq_for_tests(&self) -> u64 {
        *lock(&self.shared.gate)
    }
}

impl Drop for CursorMover {
    fn drop(&mut self) {
        {
            let mut state = lock(&self.shared.mailbox);
            state.shutdown = true;
            state.slot = None;
        }
        self.shared.ready.notify_all();
        if let Some(join) = self.join.take() {
            // Bounded by one in-flight ioctl (~1 vblank on nvidia,
            // microseconds elsewhere).
            let _ = join.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn crtc(n: u32) -> crtc::Handle {
        ::drm::control::from_u32(n).unwrap()
    }

    fn target(seq: u64, x: i32, y: i32) -> CursorTarget {
        CursorTarget {
            seq,
            x,
            y,
            hot_x: 0,
            hot_y: 0,
        }
    }

    /// Latest-wins: the mailbox is a single slot, so three posts between two
    /// takes yield only the newest. This is the same semantic
    /// `cursor_pending_move` already has — X11 motion is about where the
    /// pointer IS, not the path it took.
    #[test]
    fn mailbox_take_yields_only_the_newest_target() {
        let mut slot = None;
        for (i, x) in [10, 20, 30].into_iter().enumerate() {
            slot = Some(target(i as u64 + 1, x, 0));
        }
        let taken = take_applicable(&mut slot, 0).expect("newest target");
        assert_eq!((taken.seq, taken.x), (3, 30));
        assert!(slot.is_none(), "take must clear the slot");
        assert!(take_applicable(&mut slot, taken.seq).is_none());
    }

    /// Sequence monotonicity — the show/rebind jump-back regression. A target
    /// the worker took before a core-thread `show` must be dropped, not
    /// applied on top of the position the show just established.
    #[test]
    fn target_at_or_below_last_applied_is_dropped() {
        let mut slot = Some(target(7, 100, 100));
        assert!(
            take_applicable(&mut slot, 7).is_none(),
            "seq == last_applied"
        );
        let mut slot = Some(target(6, 100, 100));
        assert!(
            take_applicable(&mut slot, 7).is_none(),
            "seq < last_applied"
        );
        let mut slot = Some(target(8, 100, 100));
        assert!(
            take_applicable(&mut slot, 7).is_some(),
            "seq > last_applied"
        );
    }

    /// Snapshot generation: a worker holding generation N against a published
    /// N+1 must re-read before issuing.
    #[test]
    fn snapshot_staleness_is_generation_ordered() {
        assert!(snapshot_is_stale(1, 2));
        assert!(!snapshot_is_stale(2, 2));
        assert!(
            !snapshot_is_stale(3, 2),
            "a held generation ahead of the published one is not stale"
        );
    }

    /// The move to a thread must not also be a change in geometry: the
    /// per-CRTC targets computed from (snapshot, target) are exactly
    /// `cursor_root_to_crtc_local` per CRTC, which is what
    /// `try_cursor_plane_move_inner` computed inline. Dual-head, side by
    /// side: left at x=0, right at x=2560.
    #[test]
    fn crtc_targets_match_per_crtc_root_to_local() {
        let snapshot = CursorCrtcSnapshot {
            generation: 1,
            crtcs: vec![(crtc(11), 0, 0), (crtc(12), 2560, 0)],
        };
        let t = CursorTarget {
            seq: 1,
            x: 2600,
            y: 300,
            hot_x: 7,
            hot_y: 9,
        };
        assert_eq!(
            crtc_targets(&snapshot, &t),
            vec![(crtc(11), 2593, 291), (crtc(12), 33, 291)],
        );
    }

    /// An empty snapshot (VT-leave / everything hidden) issues nothing —
    /// the worker must not touch a CRTC the core thread has torn down.
    #[test]
    fn empty_snapshot_yields_no_targets() {
        let snapshot = CursorCrtcSnapshot::default();
        assert!(crtc_targets(&snapshot, &target(1, 10, 10)).is_empty());
    }

    /// Moved from platform.rs with the function.
    #[test]
    fn cursor_root_to_crtc_local_subtracts_hotspot() {
        assert_eq!(
            cursor_root_to_crtc_local(200, 300, 10, 20, 7, 9),
            (183, 271)
        );
    }

    fn stub_mover() -> CursorMover {
        let device = Arc::new(crate::drm::Device::for_tests().expect("/dev/null device"));
        CursorMover::spawn(device)
    }

    /// The worker drains posts and advances `last_applied` even when every
    /// ioctl fails (the fixture device is /dev/null → ENOTTY). Ioctl failure
    /// is logged, never fatal, and never leaves the mailbox wedged.
    #[test]
    fn worker_drains_posts_against_a_stub_device() {
        let mover = stub_mover();
        mover.publish_snapshot(vec![(crtc(11), 0, 0)]);
        for i in 0..8 {
            mover.post(i * 10, i * 10, 0, 0);
        }
        // The worker is free-running; give it a bounded window to catch up.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while mover.applied_seq_for_tests() < 8 && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(
            mover.applied_seq_for_tests(),
            8,
            "worker applied every post"
        );
        assert_eq!(mover.take_ebusy(), 0, "ENOTTY is not EBUSY");
    }

    /// `with_ioctl_gate` advances `last_applied` past every target posted
    /// before it, so a target the worker has not yet applied cannot land on
    /// top of the position the gated ioctl established — the show/rebind
    /// jump-back regression.
    ///
    /// Note what this test must NOT do: `applied_seq_for_tests()` locks the
    /// gate, so calling it from inside the `with_ioctl_gate` closure
    /// deadlocks on a non-reentrant mutex. Read it after the gate returns.
    #[test]
    fn ioctl_gate_supersedes_earlier_posts() {
        let mover = stub_mover();
        mover.publish_snapshot(vec![(crtc(11), 0, 0)]);
        mover.post(10, 10, 0, 0);
        mover.with_ioctl_gate(|| {});
        let seq_after_gate = mover.applied_seq_for_tests();
        assert!(
            seq_after_gate >= 2,
            "the gate allocated a sequence of its own"
        );
        // Whether the worker applied the earlier post before the gate or is
        // only reaching it now, `last_applied` must not move again: a target
        // at or below it is dropped.
        std::thread::sleep(std::time::Duration::from_millis(100));
        assert_eq!(
            mover.applied_seq_for_tests(),
            seq_after_gate,
            "no post older than the gate may advance last_applied afterwards"
        );
    }

    /// Dropping the mover shuts the worker down and joins it. A test that
    /// hangs here is the bug.
    #[test]
    fn drop_shuts_the_worker_down() {
        let mover = stub_mover();
        mover.post(1, 1, 0, 0);
        drop(mover);
    }
}
