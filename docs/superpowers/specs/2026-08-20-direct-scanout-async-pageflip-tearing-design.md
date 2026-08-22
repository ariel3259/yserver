# Direct Scanout with Async Page Flip (Tearing) Design Specification

- **Date:** 2026-08-20 (Updated after Adversarial Review)
- **Status:** Approved for Implementation
- **Branch:** `feat/direct-scanout-async-tearing`
- **Related Specs & Docs:**
  - `docs/high-level-design.md`
  - `AGENTS.md`
  - `docs/superpowers/specs/2026-08-12-direct-scanout-latest-wins-supersession-design.md`
  - `docs/superpowers/findings/2026-08-11-cs2-fullscreen-novsync-pageflip-collapse.md`
  - `docs/superpowers/plans/2026-08-12-direct-scanout-latest-wins-supersession.md`

---

## 1. Problem Statement

In `feat/direct-scanout-latest-wins`, Direct Scanout was stabilized against no-vsync presentation floods by integrating `scanout_m2.pending.is_some()` into `present_flip_in_flight()` and implementing a direct-level `queued` latest-wins slot. While this eliminated direct/composed thrash and held display refreshes at a stable 60.0 Hz, it enforced a **strictly VBlank-synchronized presentation pipeline** (`PAGE_FLIP_EVENT` without `PAGE_FLIP_ASYNC`).

Under this VBlank-locked direct pipeline:
1. Every presented frame is pinned to the CRTC until the next vertical blanking interval (16.66 ms at 60 Hz).
2. The client's swapchain buffers remain locked for at least one full VBlank cycle before being released to the client.
3. Consequently, the game engine's internal render loop is capped at:
   $$\text{FPS}_{\text{max}} \approx \text{Swapchain Depth} \times \text{Refresh Rate} \approx 3\text{ to }4 \times 60\text{ Hz} \approx 180\text{–}240\text{ FPS}$$
4. The game cannot produce screen tearing or achieve its GPU-unconstrained framerate (e.g. 400+ FPS).

When a game or client explicitly requests tearing / unconstrained presentation (e.g. `VK_PRESENT_MODE_IMMEDIATE_KHR` via `PresentOptionAsync` / `PresentOptionAsyncMayTear`), Direct Scanout must perform **immediate, hardware-level Async Page Flips** to achieve full framerates with tearing, while retaining the VBlank-synchronized `latest-wins` behavior for synced presentations (`VK_PRESENT_MODE_FIFO_KHR`).

---

## 2. Goals & Non-Goals

### Goals
- **Uncapped Framerate:** Enable fullscreen games with VSync disabled to cycle swapchain buffers at the GPU's native framerate (400+ FPS) without VBlank throttling.
- **Hardware Tearing Scanout:** Use `AtomicCommitFlags::PAGE_FLIP_ASYNC` when available to update scanout mid-frame.
- **Advertise `async_may_tear` in Capabilities:** Return `async_may_tear: true` in `present_capabilities` when DRM async flip capability is present so clients negotiate tearing and options bits are not masked off in core.
- **Immediate Buffer Recycling:** Release superseded and retired direct frames immediately upon async commit completion to prevent swapchain exhaustion.
- **Preserve VSync Quality:** Keep the existing `latest-wins` VBlank-synchronized path completely intact and bit-for-bit identical for VSync-enabled presentations.
- **Graceful Fallback:** If the driver rejects `PAGE_FLIP_ASYNC` (e.g., multi-output spanned CRTCs or pre-6.8 kernel in-fence restriction), fall back gracefully to synchronous direct flip, and then to composed copy if needed.

### Non-Goals
- Global tearing for desktop windows or composited windows (tearing is strictly scoped to fullscreen authoritative-root Direct Scanout; `high-level-design.md` tear-free default is preserved).
- Modifying software cursor fallback constraints (Direct Scanout still requires hardware cursor).

---

## 3. Architecture & Detailed Design

```
+-----------------------------------------------------------------------------------+
|                              Present Request Flow                                 |
+-----------------------------------------------------------------------------------+
                                         |
                                         v
                     +---------------------------------------+
                     | Is Candidate Direct Scanout Eligible? |
                     +---------------------------------------+
                                  |              |
                             Yes  |              | No
                                  v              v
               +----------------------+   +-----------------------+
               | Is Candidate Async?  |   | Composed Copy Path    |
               +----------------------+   +-----------------------+
                   |              |
          Yes      |              | No (Synced / VSync ON)
                   v              v
+-----------------------------+ +-----------------------------+
| Direct Async Scanout        | | Direct Synced Scanout       |
| - Atomic PAGE_FLIP_ASYNC    | | - Atomic PAGE_FLIP_EVENT    |
| - Immediate Tearing Flip    | | - VBlank-Locked (Latest-Win)|
| - Sub-ms Buffer Release     | | - 60 Hz Paced Frame Cycle   |
| - 400+ FPS Swapchain Loop   | | - Clean Tear-Free Motion    |
+-----------------------------+ +-----------------------------+
```

### 3.1. DRM Driver Capability Probing (`crates/yserver/src/drm/modeset.rs`)

The DRM subsystem probes driver support for async page flips at initialization:
1. Query `DRM_CAP_ATOMIC_ASYNC_PAGE_FLIP` (cap ID `21`) and `DRM_CAP_ASYNC_PAGE_FLIP` (cap ID `7`) on the control device.
2. Store `atomic_async_page_flip_supported: bool` in `DrmDeviceState` / `KmsPlatform`.
3. Validated on hardware: NVIDIA proprietary driver (610.x+) and AMD GPU (`amdgpu`) both advertise `ATOMIC_ASYNC_PAGE_FLIP = 1`.

### 3.2. Present Capabilities Advertising (`crates/yserver/src/kms/render/backend.rs`)

`backend.present_capabilities()` must reflect actual driver capabilities:
```rust
fn present_capabilities(&self, _window: u32) -> PresentCaps {
    PresentCaps {
        flip_path: self.kms_outputs_active,
        async_may_tear: self.platform.atomic_async_page_flip_supported,
        syncobj: self.dri3_capabilities().syncobj,
    }
}
```
This ensures `process_request.rs` preserves `PRESENT_OPTION_ASYNC_MAY_TEAR (0x10)` on incoming requests instead of silently stripping it.

### 3.3. Atomic Commit with Async Page Flip (`crates/yserver/src/drm/modeset.rs`)

`submit_direct_scanout` is extended with an `async_flip: bool` parameter:

```rust
pub(crate) fn submit_direct_scanout(
    device: &Device,
    fb: framebuffer::Handle,
    planes: &[DirectScanoutPlaneState<'_>],
    async_flip: bool,
) -> io::Result<()> {
    if planes.is_empty() {
        return Err(io::Error::other("scanout M2: empty plane transaction"));
    }
    let mut request = AtomicModeReq::new();
    for state in planes {
        // ... (populate plane properties: fb_id, crtc_id, src/crtc geometry)
    }

    // Atomic async page flips are only supported on single-CRTC transactions by DRM kernel
    let can_async = async_flip && planes.len() == 1;
    let flags = direct_scanout_commit_flags(can_async);

    match device.atomic_commit(flags, request.clone()) {
        Ok(()) => Ok(()),
        Err(err) if can_async => {
            log::debug!("atomic async page flip declined ({err}), falling back to sync direct commit");
            let fallback_flags = direct_scanout_commit_flags(false);
            device.atomic_commit(fallback_flags, request)
        }
        Err(err) => Err(err),
    }
}
```

### 3.4. State Machine & Buffer Lifecycle (`crates/yserver/src/kms/render/backend.rs`)

1. **`DirectPresentFrame` Structure:**
   - Record `is_async: bool` inside `DirectPresentFrame`, evaluated from:
     ```rust
     let is_async = (candidate.options & (PRESENT_OPTION_ASYNC | PRESENT_OPTION_ASYNC_MAY_TEAR)) != 0;
     ```

2. **Presentation Admission in `try_present_direct`:**
   - When `pending.is_none()`:
     - Execute `submit_direct_scanout(..., frame.is_async)`.
     - Assign `scanout_m2.pending = Some(frame)`.
     - Assign `scanout_m2.pending_is_submitted = true`.
   - When `pending.is_some()` (an async flip is currently completing in the kernel):
     - If `queued` exists, complete previous `queued` frame as `Skip` and release its pins/buffers immediately.
     - Store incoming frame in `scanout_m2.queued`.
     - Return `Ok(true)` (no Copy fallback, no direct unflip).

3. **Retirement & Immediate Buffer Wake in `retire_direct_output`:**
   - When the hardware page flip event arrives:
     - `retired = scanout_m2.pending.take()`.
     - `retired.event.completion_mode = COMPLETE_MODE_FLIP`.
     - If `scanout_m2.current` exists, call `release_direct_frame(previous)` immediately:
       - Pushes to `scanout_m2.idled`.
       - Releases drawable pins.
       - Signals `signal_present_wake` (DRI3 fence / Vulkan release syncobj).
     - Store `scanout_m2.current = Some(retired)`.
     - If `queued` has a frame waiting, promote `queued -> pending` (`pending_is_submitted = false`) and trigger `submit_chain_direct_frame` on the next loop tick.

---

## 4. State Transitions

| State | Event | Action | Next State | Buffer Action |
|---|---|---|---|---|
| `pending=None`, `queued=None` | Arrive Async Present | Submit `PAGE_FLIP_ASYNC` | `pending=Async(InFlight)`, `queued=None` | Source pinned, wake retained |
| `pending=Async(InFlight)`, `queued=None` | Arrive Async Present | Store into `queued` | `pending=Async(InFlight)`, `queued=New` | New source pinned |
| `pending=Async(InFlight)`, `queued=Old` | Arrive Async Present | Complete `Old` as `Skip`, store `New` | `pending=Async(InFlight)`, `queued=New` | `Old` released immediately |
| `pending=Async(InFlight)`, `queued=Frame` | Async Flip Event Arrives | `pending -> current`, promote `queued -> pending` | `pending=Promoted`, `current=Retired` | Previous `current` released immediately, chain submit armed |
| `pending=Promoted`, `queued=None` | Loop Tick (`maybe_composite`) | Submit chain `PAGE_FLIP_ASYNC` | `pending=Async(InFlight)` | Frame in flight |

---

## 5. Verification & Acceptance Criteria

### Unit Test Coverage (TDD)
- **Predicate Tests:**
  - Verify `direct_scanout_commit_flags(is_async: true)` produces `PAGE_FLIP_EVENT | PAGE_FLIP_ASYNC | NONBLOCK`.
  - Verify `direct_scanout_commit_flags(is_async: false)` produces `PAGE_FLIP_EVENT | NONBLOCK`.
  - Verify `present_capabilities` advertises `async_may_tear: true` when driver capability is active.
- **State Machine Transitions:**
  - Verify `complete_queued_as_skip` releases buffers immediately in async mode.
  - Verify promotion and immediate release of `current` upon async page flip completion.

### Hardware Validation
- **Environment:** NVIDIA RTX 5060 Ti (`card1`, driver 610.57.04) & AMD Raphael (`card0`, amdgpu).
- **Target Application:** CS2 / Marvel Rivals with VSync disabled in-game.
- **Metrics:**
  1. In-game FPS counter reaches unconstrained rate (300–400+ FPS), eliminating the 50% drop.
  2. Screen tearing visibly observable (confirming immediate mid-scanout line switching).
  3. `page_flip/s` reflects high-cadence async commits without event loop exhaustion.
  4. 0 unexpected composed unflips in steady state.
  5. VSync ON mode retains clean 60.0 Hz tear-free pacing.
- **Code Quality:**
  - Clean `cargo clippy --all-targets -- -D warnings`.
  - Formatted with `cargo +nightly fmt`.

---

## 6. Architectural Discoveries & Hardware Pipeline Hardening

During real-world hardware validation on uncapped high-framerate Vulkan titles (Counter-Strike 2, Metal Gear Solid V) on NVIDIA RTX hardware, several subtle architectural bottlenecks and driver contention points were identified and resolved to achieve 100% smooth real-time tearing pacing.

### 6.1. Bypassing User-Space Sync-File Export & Epoll Deferral in Direct Scanout

- **Root Cause & Xorg Parity:**
  In Xorg (`present_execute.c:86`), page flip synchronization is explicitly delegated to the DRM driver (*"Flip synchronization is managed by the driver"*).
  In `yserver`, `arm_present_source_wait` previously exported dma-buf sync files (`DMA_BUF_IOCTL_EXPORT_SYNC_FILE`) on every `PresentPixmap` and parked frames in `present_pending_exec` waiting on epoll when the GPU was actively rendering. Under intense scene rendering (e.g. multi-player gunfights), this delayed `try_present_direct` from being called upon arrival, holding the client's previous buffer from being idled and collapsing the Vulkan swapchain from 400 FPS down to 1–11 FPS.
- **Architectural Solution:**
  When `self.scanout_m2.active()` is true, `arm_present_source_wait` and `arm_present_syncobj_wait` immediately return `Ok(PresentSourceWait::Ready)`. The frame is committed directly to KMS on arrival, where the kernel DRM driver (`drm_atomic_helper_wait_for_fences`) synchronizes the display plane with the GPU's memory reservation at hardware level, and the previous buffer is idled back to Vulkan immediately.

### 6.2. Elimination of Hardware Cursor Contention & XFixes Hide/Show Implementation

- **Root Cause:**
  On `nvidia-drm`, the legacy cursor movement ioctl (`drmModeMoveCursor`) contends for the internal CRTC display channel mutex against atomic page flips (`atomic_commit(PAGE_FLIP_ASYNC)`). Fullscreen games render in-engine crosshairs and invoke `XFixesHideCursor` or define an empty/None cursor. Because `XFixesHideCursor` was a stub and `refresh_effective_cursor` did not detach the hardware plane on `None` cursors, 1000 Hz gaming mice triggered 1000 legacy cursor ioctls/second against the active scanout CRTC, causing noticeable micro-stutter and frame judder ("trash") during camera rotation.
- **Architectural Solution:**
  1. **Full `XFixesHideCursor` / `XFixesShowCursor` Implementation:** Unbinds the KMS cursor plane (`cursor_plane_hide_all`) immediately when requested by the game.
  2. **`None` Cursor Unbinding:** `refresh_effective_cursor` clears the scene cursor and calls `cursor_plane_hide_all` when `new_xid` is `None`.
  3. **Cursor Coordinate Deduplication:** `CursorPlane::move_to` caches `last_pos` per CRTC and suppresses redundant kernel ioctls.

### 6.3. Immediate Socket Flush & Event Loop Tail Reordering

- **Root Cause:**
  Present completion and idle notifications queued to client output buffers were occasionally delayed across epoll poll timeouts when `reconcile_client_writable_interest` executed before `run_iteration_tail`.
- **Architectural Solution:**
  `run_iteration_tail` is executed prior to writable interest reconciliation in `run.rs`, ensuring `PresentCompleteNotify` and `PresentIdleNotify` packets are written to the client's UNIX domain socket in the exact same iteration the frame is retired.

### 6.4. Fast-Path KMS Async Atomic Property Trimming

- When `can_async` is true, `submit_direct_scanout` fast-paths atomic property generation by submitting only `FB_ID` and `CRTC_ID`, omitting 8 redundant geometry properties to avoid driver-side atomic state validation overhead.
- Synchronous fallback retry on async commit rejection was removed to prevent transient driver busy states from demoting the server into 60 Hz VSync.

### 6.5. Movement Key Repetition in Proton / Wine

- `fire_pending_repeats` was updated to emit pure `KeyPress` events without synthetic `KeyRelease` pulses, ensuring continuous movement inputs (e.g. holding 'W' to run in MGSV) remain uninterrupted without motion cancellation.

### 6.6. Fullscreen Redirected Windows Admission & Unflip Stabilization

- **Root Cause:**
  When desktop window managers (e.g. Cinnamon/Muffin) redirect fullscreen windows, the window has `redirected = true` while covering the entire root window. Previously, direct scanout rejected redirected windows, forcing composed copy and causing desktop background flicker when the window manager transitioned states.
- **Architectural Solution:**
  1. `scanout_candidate_from_window` admits fullscreen redirected windows that cover the full root geometry.
  2. Removed premature direct unflip requests on transient compositor state changes, ensuring direct scanout remains stable without unflip thrash.

---

## 7. Dual-GPU PRIME Render Offload Architecture

During hardware testing with hybrid GPU setups (AMD Raphael integrated GPU + NVIDIA GeForce RTX 5060 Ti dedicated GPU):
- **Display Server (`yserver`):** Runs on the open-source `amdgpu` driver using standard Linux DRM/KMS, Atomic Modesetting, GBM, and DMA-BUF.
- **3D Render Offload:** Vulkan and OpenGL applications (CS2, DXVK, Proton) select the discrete NVIDIA GPU as the Vulkan physical device / render node (`/dev/dri/renderD129` / `/dev/nvidia0`).
- **Presentation:** Rendered swapchain buffers are presented through standard DRI3 / DMA-BUF to `yserver` on the AMD display output.
- **Benefit:** Provides 100% raw compute/rendering performance of the discrete NVIDIA GPU while bypassing proprietary `nvidia-drm` KMS modesetting and cursor ioctl limitations.

