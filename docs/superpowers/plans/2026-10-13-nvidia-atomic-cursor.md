# NVIDIA hardware cursor in the wlroots model — implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or
> superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** on nvidia-drm, drive the hardware cursor through atomic commits in the wlroots model so the NVIDIA
software-cursor policy can go, and let direct scanout engage when the cursor is hidden — without a KMS thread.

**Architecture:** one commit stream per CRTC with at most one commit in flight. The cursor plane's `CRTC_X`/`CRTC_Y`
ride every scene commit on that CRTC (shared flip, copied flip, direct scanout, composed unflip). When nothing is in
flight on the CRTC, motion sends a cursor-only `NONBLOCK | PAGE_FLIP_EVENT` commit, and further motion coalesces
(latest wins) until it retires. While a cursor-only commit is in flight, the scene and direct scanout wait for it
exactly as they wait for their own flip. Other drivers keep master's legacy cursor ioctls unchanged.

**Tech Stack:** Rust, `drm` crate atomic requests, nvidia-drm (open kernel modules), the 3b `OutputSink` /
`CursorPlaneSink` seam (`crates/yserver-render/src/kms/render/scene/sink.rs`).

**Evidence (measured 2026-10-13 on card1, spike on master 7b5fff83, branch `spike/nvidia-atomic-cursor`):**
software cursor, legacy hardware cursor and this model were compared on pure motion, window drag, windowed mpv plus
motion and fullscreen mpv plus motion. This model matched the software cursor on every test (0 round trips over 8 ms,
drag p99 12 px, 60 fps, 0 mpv drops). It got 2068 direct submits where the software cursor gets 0. EBUSY was 0, and
NONBLOCK ioctl time had a p99 of 60–150 µs. The legacy cursor had 1200 stalls. Root cause of the legacy stall:
nvidia-drm has no `atomic_async_check`/`atomic_async_update`, and its own `nv_drm_atomic_commit` ignores
`legacy_cursor_update`, so a legacy cursor ioctl is a blocking commit that waits for the pending flip and its own
vblank (`nvidia-drm-modeset.c:746`, `:897`). The spike diff is
`~/Projects/claude-scratch/yserver-pr1/bin-spike/spike-final.diff` (reference only — its scene parts predate 3b).

## Global Constraints

- Behaviour changes only for devices where `drm_device_is_nvidia` is true; every other driver issues exactly master's
  legacy `set_cursor2` / `move_cursor` calls (a test pins this).
- No environment variable switches behaviour (project rule). `YSERVER_LOOP_TELEMETRY` stays a pure diagnostic.
- No `cfg(test)` forks or test-only behaviour in production bodies; test seams are `#[cfg(test)]` items production does
  not call, or constructor parameters. No side effects inside `debug_assert!`. No sleeps to order events.
- Test rules: **A** end-state checks (no cursor commit left in flight, no unsent position, the recorded commit
  sequence exact); **B** advance only through `Backend` trait entries or `yserver_core::core_loop::run_core`;
  **C** hardware validation per real-path task (Task 5); **F** each new test 5 times under 16 `nproc` busy loops
  started outside the tests and killed afterwards.
- Every test names the mutation that must make it fail; run it once.
- Show/hide and image upload keep master's legacy ioctls (`set_cursor2`); only positions go atomic.
- EBUSY on any commit of this stream is a bookkeeping bug: count it, warn, and handle it the way master handles a
  failed flip or move today. No retry loops.
- The intermediate-disable RANDR stall (~3.1 s in `disable_output`, present with every cursor mode) is out of scope.

## Review Focus

1. **A cursor-only commit in flight when the topology changes or the VT is released.** Its flip event must be
   drained by the existing old-event discard, never attributed to the next scene flip on a reused CRTC. Add cursor
   in-flight CRTCs to `pending_pageflip_crtcs` (`backend/kms/crtc_config.rs:42`) and the VT path's equivalent (Task 3
   test).
2. **A RANDR mode change passing through zero active CRTCs.** The plane-to-CRTC position properties are discovery
   data and must survive `CursorPlane::retain_crtcs` (the spike fell back to the software cursor here; Task 1 test).
3. **Motion while a direct-scanout successor or unflip is pending.** Nothing may be submitted to the CRTC, and the
   position must ride the next direct or composed commit (Task 3 test).
4. **The cursor shown again during direct scanout after a game hid it.** The existing path must bind the plane or
   unflip; the cursor must never be invisible while shown (Task 4 test).
5. **Cursor on a copied (PRIME) output.** The position rides the copied flip on the sink device's CRTC, with the sink
   device's cursor plane (Task 2 test).

---

### Task 1: Atomic position properties and a recordable commit sink

**Files:** `crates/yserver/src/kms/cursor_plane.rs`, new `crates/yserver/src/drm/atomic.rs` (+ `drm/mod.rs`),
`crates/yserver/src/kms/render/platform/cursor.rs`, `platform/init.rs`.

**Interfaces — produces:**
- `CursorPlane::atomic_position_properties(crtc) -> Option<(plane::Handle, property::Handle, property::Handle)>`
  and `supports_atomic_positions(&[crtc]) -> bool`. Filled at discovery from every cursor plane with `CRTC_X` and
  `CRTC_Y`; **`retain_crtcs` never prunes them.**
- `crate::drm::atomic::CursorPlanePosition { crtc, plane, crtc_x, crtc_y, x, y }` with
  `add_to(&self, req: &mut AtomicModeReq)`.
- An `AtomicCommitSink` held per KMS device (constructor parameter; production = the device's `atomic_commit`; the
  `#[cfg(test)]` recorder stores each request's object/property/value triples and flags). It records the commit kind
  (`cursor-only`, `scene-with-cursor`, `scene`), ioctl time and result. Debug log
  `atomic commit kind=… crtc=… ioctl_us=… result=…`; a per-second info line under `YSERVER_LOOP_TELEMETRY`:
  per-kind counts, EBUSY, max/p99 ioctl µs.
- `KmsCursorState::new_with_atomic_cursor(nvidia: bool)` replaces `new_with_nvidia_policy`; on NVIDIA,
  `topology_blocked` is also true when a CRTC has no atomic position properties (software fallback, as today).

- [ ] Test `nvidia_cursor_keeps_atomic_positions_across_a_zero_crtc_topology_step`: install, `retain_crtcs(∅)`,
  `retain_crtcs({crtc})` → `supports_atomic_positions` true, `is_visible_on` false. Mutation: prune
  `atomic_positions` in `retain_crtcs`.
- [ ] Test: a plane without `CRTC_X` on NVIDIA blocks the topology (software cursor). Mutation: drop the check.
- [ ] Gate (below); commit `feat(kms): atomic cursor position properties and a recordable commit sink`.

### Task 2: The cursor rides every scene commit

**Files:** `crates/yserver/src/kms/render/platform/scene_sink.rs` (shared flip, ~L440),
`platform/scanout_bos.rs` (copied flip, ~L123), `backend/kms/scanout.rs` (direct ~L534, composed unflip ~L854),
`crates/yserver/src/drm/{page_flip.rs,modeset.rs}`.

**Interfaces:** `PlatformBackend::cursor_plane_atomic_position_for_output(output_idx) -> Option<CursorPlanePosition>`
(NVIDIA device, plane visible on that CRTC, latest position); the four submit functions take
`Option<&CursorPlanePosition>` and append it; on success `cursor_plane_scene_commit_submitted(output_idx)` clears that
CRTC's unsent position.

- [ ] Test (shared flip, through `tick_maybe_composite_for_tests` or the scripted scene path): the recorded request
  carries the cursor plane's `CRTC_X`/`CRTC_Y` = the latest position. Mutation: omit the append.
- [ ] Test: direct scanout and composed unflip requests carry the position. Mutation: omit it in one of them.
- [ ] Test (copied, Review Focus 5): the copied flip carries the sink device's cursor plane position. Mutation: use
  the renderer device's plane.
- [ ] Test: non-NVIDIA scene commits carry no cursor properties. Mutation: append unconditionally.
- [ ] Gate; commit `feat(kms): the NVIDIA cursor position rides the scene's atomic commit`.

### Task 3: Cursor-only commits, retirement and the shared gate

**Files:** `platform/cursor.rs`, `platform/flip_events.rs`, `backend/portable/pointer.rs`, `backend/kms/scanout.rs`,
`backend/kms/crtc_config.rs`, `backend/kms/session.rs`, `crates/yserver-render/src/kms/render/scene/{sink.rs,tick.rs}`.

**Interfaces / invariants:**
- Per CRTC: `atomic_pending_moves` (latest wins), `atomic_cursor_commits_in_flight`. Motion on NVIDIA stores the
  position; if the CRTC has no scene flip awaiting retirement, no direct/unflip transaction pending and no cursor-only
  commit in flight, it submits a cursor-only commit and marks it in flight.
- A flip event on a CRTC with a cursor-only commit in flight retires that commit (at most one commit in flight per
  CRTC, so the CRTC identifies it). After retirement the backend's next scheduling pass sends the unsent position as
  a cursor-only commit only if the scene has nothing ready for that output; otherwise it rides the scene commit.
- New `OutputSink::output_commit_in_flight(output_idx) -> bool` (default `false` for `TestSink` unless set). The
  scene's per-output submit gate (`tick_one_output`) and `has_output_ready_for_submit` treat it like a non-empty
  `pending_acks`. Direct submission (`try_present_direct` path) and the composed unflip wait while any cursor-only
  commit is in flight on their outputs.
- `pending_pageflip_crtcs` (and the VT-release path's set) include CRTCs with a cursor-only commit in flight, so the
  old-event discard drains them (Review Focus 1). `cursor_plane_hide_all` and topology changes clear the per-CRTC
  state only after that discard.
- The NVIDIA software-cursor policy is deleted.

- [ ] Test `nvidia_idle_motion_coalesces_until_cursor_flip_retires` (via `Backend::warp_pointer_root` and
  `on_page_flip_ready`): one cursor-only commit, then nothing until retirement, then one commit with the latest
  position. Mutation: first-wins insertion.
- [ ] Test: motion during a pending scene flip sends nothing; the next scene request carries the latest position.
  Mutation: ignore the scene blocker.
- [ ] Test: the scene does not submit while a cursor-only commit is in flight and submits after its retirement.
  Mutation: `output_commit_in_flight` always false.
- [ ] Test (Review Focus 3): motion while a direct successor is pending sends nothing; the successor carries it.
  Mutation: drop the direct blocker.
- [ ] Test (Review Focus 1): a cursor-only commit in flight at a RANDR change — its event is discarded, the first
  scene flip after the change retires the scene's own frame. Mutation: leave cursor CRTCs out of
  `pending_pageflip_crtcs`.
- [ ] Test `non_nvidia_pointer_motion_keeps_one_legacy_move_and_no_atomic_commit`. Mutation: route non-NVIDIA through
  the atomic branch.
- [ ] Gate; commit `feat(kms): NVIDIA cursor-only atomic commits, one commit in flight per CRTC`.

### Task 4: A hidden cursor does not block direct scanout

**Files:** `crates/yserver-render/src/kms/render/scene/cursor.rs` (`classify_cursor_mode_from_per_output` L28,
`cursor_mode` L114), the `CursorPlaneMode` enum (`yserver-render/src/kms/render/scene.rs:275`),
`crates/yserver/src/kms/render/backend/kms/scanout.rs` (`scanout_m1_probe_eligible` calls ~L1013 and the
diagnostic one ~L1465, composite-tick unflip ~L2034), `backend/portable/pointer.rs` (mode match).

**Cause:** XFIXES `HideCursor` makes every output `OutputCursorMode::Hidden`; the classifier maps "no Hw and no Sw"
to `CursorPlaneMode::Sw`, and both direct gates require `Hw`, so a hidden cursor — which needs no composition — blocks
direct scanout on every GPU (the unflip reason is literally `composite_tick_software_or_hidden_cursor`).

**Invariant:** a cursor that is hidden on every output is direct-compatible. Add `CursorPlaneMode::Hidden` (all
outputs `Hidden`, no pending transition); both direct gates accept `Hw | Hidden`; the pointer fast path treats
`Hidden` as "nothing to move". `Mixed` stays as today.

- [ ] Test (render crate, `TestSink`): all outputs hidden → `cursor_mode() == Hidden`; Hidden + Sw → `Sw`;
  Hidden + Hw → `Hw`. Mutation: map all-Hidden to `Sw`.
- [ ] Test (backend): after `HideCursor`, an eligible fullscreen Present enters direct scanout and the composite tick
  does not request an unflip. Mutation: gate on `Hw` only.
- [ ] Test (Review Focus 4): `ShowCursor` during direct scanout binds the plane (Hw) or unflips (Sw); never leaves the
  cursor invisible. Mutation: skip the bind on show.
- [ ] Gate; commit `fix(kms): a hidden cursor does not keep direct scanout off`.

### Task 5: Hardware validation (card1, with the user's approval)

Run `~/Projects/claude-scratch/yserver-pr1/hw_spike_matrix.sh` with master (software cursor), master with the policy
lifted (legacy) and this branch. Then fullscreen mpv with the cursor hidden (`xdotool` / an XFIXES HideCursor client),
a RANDR mode change while moving, and a VT switch by a person from a tty3 login. Stop at the first Xid or `ERR!`.
Acceptance: each matrix row equals the software cursor (0 round trips > 8 ms, drag p99 within 1 px, 60 fps, 0 drops);
direct submits > 0 in fullscreen with the cursor visible and with it hidden; EBUSY 0; NONBLOCK ioctl p99 < 1 ms; the
cursor stays hardware after the RANDR change; the VT switch restores the cursor. Record the table in this plan and in
`docs/status.md`.

## Gate (every task)

1. `cargo +nightly fmt`, then `cargo +nightly fmt -- --check`.
2. Clippy `--workspace --all-targets -D warnings`: default, `--features yserver/tcp-transport`,
   `--features yserver/xdmcp`.
3. `cargo test --workspace --locked`, and each file under `crates/yserver/tests/` by name.
4. The lavapipe ignored suite in the form master's CI uses (`.github/workflows`), with
   `VK_DRIVER_FILES=~/Projects/claude-scratch/yserver-pr1/lvp/lvp_icd.json YSERVER_ALLOW_SOFTWARE_VULKAN=1`;
   `two_gpu`/`_drm` tests are skipped without the user's approval.
5. Rule F for the task's new tests.
6. Cargo.lock unchanged. FreeBSD: "not checked" (CI runs it).

## Non-goals

The KMS owner thread; the RANDR intermediate-disable stall; tearing / async page flips; atomic show/hide or cursor
image changes; the software clock; any change for non-NVIDIA drivers.
