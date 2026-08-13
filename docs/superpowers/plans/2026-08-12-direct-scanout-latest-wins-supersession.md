# Direct-Level Latest-Wins Supersession Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Kill the direct/composed thrash of the Phase B direct-scanout path under a no-vsync fullscreen synced-present flood (CS2), so `page_flip/s` holds at refresh (~60) and composed unflips drop to ~0/s.

**Architecture:** Two pieces. (1) `present_flip_in_flight()` learns to see the in-flight direct frame (`scanout_m2.pending`), so the existing core parking + synced same-target supersession coalesce the flood to ~1 present/flip before the direct path. (2) A `scanout_m2.queued` slot holds a prepared not-yet-submitted direct frame; a present arriving while the direct flip is in flight replaces it (latest-wins) instead of unflipping, and `maybe_composite` chain-flips the promoted frame on the next tick. Both pieces are scoped to the direct-scanout path; synced-present behavior on the composed path is unchanged.

**Tech Stack:** Rust, KMS/DRM atomic, Vulkan, yserver-core `Backend` trait, `yserver/src/kms/render/backend.rs` + `scene.rs`.

## Global Constraints

- Spec: `docs/superpowers/specs/2026-08-12-direct-scanout-latest-wins-supersession-design.md`.
- Synced-present behavior must be bit-for-bit unchanged on the composed path — both pieces only add conjuncts/gates on the direct-scanout path.
- TDD: failing test → run → implement → run again. CI gate at the end:
  `cargo clippy --all-targets -- -D warnings` and `cargo +nightly fmt`.
- Feature branch: `feat/direct-scanout-latest-wins` (off `fix/fullscreen-novsync-stutter`; includes the Phase B gates + C1 unflip-degradation fix + `YSERVER_HW_CURSOR_NVIDIA` override).
- Test fixtures have no Vk/DRM: `for_tests()` has no live scene outputs and no probe cache. **Never write fixture-integration tests for direct scanout submits** — test pure predicates and the `pending`/`queued` state transitions only; hardware validation is a separate session.
- Direct scanout only engages when `cursor_hw` (m1 guard) — the `YSERVER_HW_CURSOR_NVIDIA=1` override is what makes that possible on the nvidia box. This plan does not change that.
- Commit after each task.

---

## File structure

- `crates/yserver/src/kms/render/backend.rs` — the whole direct-scanout state machine: `ScanoutM2State`, `DirectPresentFrame`, `present_flip_in_flight`, `present_completion_is_idle`, `try_present_direct`, `retire_direct_output`, `maybe_composite`, `stop_direct_after_scanout_replaced`, plus the scanout test module.
- `crates/yserver/src/kms/render/scene.rs` — unchanged (only read; `has_pending_page_flips` stays scene-only).
- No new files. The `queued` slot and `pending_is_submitted` flag live inside `ScanoutM2State`; the pure predicates live next to the other scanout predicates.

Task dependencies: Task 1 (Piece 1) is independent and must land first — it alone may fix the thrash; Task 2 (Piece 2) builds on the `pending`-aware state. Tasks 3-4 are the queued slot + chain-flip. Task 5 is the hardware validation. Task 6 is the CI gate.

---

## Task 1: `present_flip_in_flight()` and `present_completion_is_idle()` see the direct flip

**Files:**
- Modify: `crates/yserver/src/kms/render/backend.rs:13627-13629` (`present_flip_in_flight`), `:7096-7098` (`present_completion_is_idle`), `:301-309` (`DirectPresentFrame` — add `fb` field now, consumed by Task 2), `:32060-32074` (existing test).
- Test: same file, extend the existing test + add a new one.

**Interfaces:**
- Consumes: `self.scanout_m2.pending: Option<DirectPresentFrame>` (exists), `self.scene.has_pending_page_flips()` (exists), `crate::drm::modeset::DirectScanoutProbeFramebuffer` (exists).
- Produces: `present_flip_in_flight() -> bool` now `true` when `scanout_m2.pending.is_some()`; `present_completion_is_idle() -> bool` now `false` when `scanout_m2.pending.is_some()`; `DirectPresentFrame.fb: Option<Arc<DirectScanoutProbeFramebuffer>>` (None on the fixture); `DirectPresentFrame::for_tests()` (cfg(test)).

- [ ] **Step 1: Add the `fb` field to `DirectPresentFrame` and the fixture constructor**

`CompletedPresentEvent` does NOT derive `Default` (it has `ClientId`, `PresentWake`, `u32`s, `u64`, `u8` — read the exact field list at `crates/yserver-core/src/backend/trait_def.rs:188`). Fill every field explicitly in the constructor. `PresentWake::Pixmap { idle_fence_xid: 0 }` is the default wake.

First, add the `fb` field to the struct AND to the **production** construction site in the same step (otherwise the crate does not compile at the end of this task — adversarial review B5):

```rust
struct DirectPresentFrame {
    source_pin: u64,
    fallback_target_pin: u64,
    source_id: DrawableId,
    candidate: PresentScanoutCandidate,
    fallback_target: PaintTarget,
    event: yserver_core::backend::CompletedPresentEvent,
    awaiting_outputs: HashSet<usize>,
    /// Retained so a queued (not-yet-submitted) frame survives an m1 cache
    /// clear (topology change) — the cache entry owns the fb and its `Drop`
    /// rm_fb's it. `None` only on the `for_tests()` fixture and (temporarily,
    /// until Task 2) on submitted frames.
    fb: Option<std::sync::Arc<crate::drm::modeset::DirectScanoutProbeFramebuffer>>,
}
```

At the production construction (backend.rs:13256) add `fb: None`:

```rust
self.scanout_m2.pending = Some(DirectPresentFrame {
    source_pin,
    fallback_target_pin,
    source_id,
    candidate,
    fallback_target,
    event,
    awaiting_outputs,
    fb: None, // Task 2 upgrades this to Some(Arc::clone(...))
});
```

Then the fixture constructor (adversarial review B4 — use `yserver_core::backend::PresentScanoutCandidate`, NOT `crate::backend`, and include the `client_id` field which is the first field of the candidate at trait_def.rs:79):

```rust
#[cfg(test)]
impl DirectPresentFrame {
    fn for_tests() -> Self {
        use crate::kms::render::store::DrawableId;
        use yserver_core::backend::{CompletedPresentEvent, PresentScanoutCandidate, PresentWake};
        Self {
            source_pin: 1,
            fallback_target_pin: 2,
            source_id: DrawableId::for_tests(9),
            candidate: PresentScanoutCandidate {
                client_id: 1,
                present_id: 1,
                src_pixmap_xid: 0x100,
                dst_window_xid: 0x200,
                src_host_xid: 0x300,
                paint_dst_host_xid: 0x400,
                completion_dst_host_xid: 0x400,
                src_width: 1920,
                src_height: 1080,
                x_off: 0,
                y_off: 0,
                valid_region_xid: 0,
                update_region_xid: 0,
                update_is_full: true,
                explicit_sync: false,
                options: 0,
            },
            fallback_target: PaintTarget {
                id: DrawableId::for_tests(10),
                offset: (0, 0),
                x11_depth: 24,
            },
            event: CompletedPresentEvent {
                client_id: yserver_protocol::x11::ClientId(1),
                serial: 1,
                host_xid: 0x400,
                dst_host_xid: 0x400,
                options: 0,
                present_id: 1,
                wake: PresentWake::Pixmap { idle_fence_xid: 0 },
                completion_mode: 0,
                emit_idle: false,
            },
            awaiting_outputs: std::collections::HashSet::new(),
            fb: None,
        }
    }
}
```

> The production fixture candidate at backend.rs:32256 (NOT 32150) shows the real `PresentScanoutCandidate` field list including `client_id: 1` — copy from there if the fields above drift. `PresentScanoutCandidate`, `CompletedPresentEvent`, `PresentWake`, `ClientId`, `DrawableId::for_tests`, `COMPLETE_MODE_SKIP` are all real and resolve as assumed (verified in review).

- [ ] **Step 2: Extend the existing test to pin the new conjuncts**

`present_flip_in_flight_mirrors_scene_state` (backend.rs:32060) currently asserts the fixture starts `!present_flip_in_flight()` and mirrors `scene.has_pending_page_flips()`. Add, after the existing assertions:

```rust
b.scene.test_set_flip_in_flight(false);
assert!(!b.present_flip_in_flight(), "no scene flip, no pending direct");
b.scanout_m2.pending = Some(super::DirectPresentFrame::for_tests());
assert!(
    b.present_flip_in_flight(),
    "a pending direct frame counts as a flip in flight"
);
b.scanout_m2.pending = None;
assert!(!b.present_flip_in_flight());
```

And add a new test next to `present_display_idle_false_when_scene_wants_compose_even_with_no_flips` (~32077):

```rust
#[test]
fn present_completion_idle_false_when_direct_pending() {
    let mut b = super::KmsBackend::for_tests();
    assert!(b.present_completion_is_idle(), "idle at init");
    b.scanout_m2.pending = Some(super::DirectPresentFrame::for_tests());
    assert!(
        !b.present_completion_is_idle(),
        "idle-display fallback must not arm while a direct flip is in flight"
    );
}
```

- [ ] **Step 3: Implement the two predicates**

```rust
fn present_flip_in_flight(&self) -> bool {
    self.scene.has_pending_page_flips() || self.scanout_m2.pending.is_some()
}

fn present_completion_is_idle(&self) -> bool {
    !self.scene.has_pending_page_flips()
        && !self.scene_wants_compose()
        && self.scanout_m2.pending.is_none()
}
```

- [ ] **Step 4: Run the tests**

Run: `cargo test -p yserver --lib present_flip_in_flight`
Run: `cargo test -p yserver --lib present_completion_idle`
Expected: both PASS.

- [ ] **Step 5: Run the wider present suites for regressions**

Run: `cargo test -p yserver --lib present_`
Run: `cargo test -p yserver-core --lib present_`
Expected: PASS (the new conjuncts are additive; the composed-path predicates are unchanged when no direct frame is pending).

- [ ] **Step 6: Commit**

```bash
git add crates/yserver/src/kms/render/backend.rs
git commit -m "feat(scanout): count the in-flight direct frame in present flip/idle predicates"
```

---

## Task 2: `DirectPresentFrame` retains the probe framebuffer by `Arc`

**Files:**
- Modify: `crates/yserver/src/kms/render/backend.rs:252-271` (`ScanoutM1ProbeEntry` — store `Arc<DirectScanoutProbeFramebuffer>`), `:1754` (accept site — wrap in `Arc`), `:13142-13164` (`try_present_direct` submit block — retain the fb), `:13250-13254` (wake-pin registration single point).
- Test: same file, a pure predicate + `direct_frame_requires_retained_framebuffer_before_queue`.

**Interfaces:**
- Consumes: `DirectPresentFrame.fb: Option<Arc<DirectScanoutProbeFramebuffer>>` (added in Task 1), `crate::drm::modeset::DirectScanoutProbeFramebuffer` (exists, `pub(crate)` struct with `handle() -> framebuffer::Handle`, `Arc<Device>` inside, so it is `Send + Sync`), `ScanoutM1ProbeEntry::framebuffer()` (backend.rs:268).
- Produces: the m1 cache entry holds `Option<Arc<DirectScanoutProbeFramebuffer>>`; production `DirectPresentFrame` instances carry `Some(Arc::clone(&fb))`; the submit uses `fb.handle()`.

- [ ] **Step 1: Add a pure retention predicate + failing test**

```rust
/// A direct frame is safe to queue only when it retains its probe
/// framebuffer (a raw handle would dangle if the m1 cache clears while the
/// frame waits — the cache entry owns the fb lifetime).
fn direct_frame_retains_framebuffer(frame_has_fb: bool) -> bool {
    frame_has_fb
}

#[test]
fn direct_frame_requires_retained_framebuffer_before_queue() {
    assert!(super::direct_frame_retains_framebuffer(true));
    assert!(!super::direct_frame_retains_framebuffer(false));
}
```

- [ ] **Step 2: Make the m1 cache entry own an `Arc` (adversarial review B3)**

`ScanoutM1ProbeEntry` (backend.rs:252) currently stores the framebuffer by value, and `DirectScanoutProbeFramebuffer` has no `Clone` (its `Drop` rm_fb's + GEM-closes; cloning the owned value would double-free). Change the cache to hold `Arc` so the frame can retain a refcounted clone that outlives a cache clear:

```rust
struct ScanoutM1ProbeEntry {
    /// Retained solely for its FB/GEM lifetime; `Drop` performs teardown.
    /// `Arc` so a queued direct frame can retain a refcounted clone across a
    /// `scanout_m1.clear` (topology change) — the cache entry dropping to zero
    /// refs rm_fb's the fb only once the frame is done with it.
    _framebuffer: Option<std::sync::Arc<crate::drm::modeset::DirectScanoutProbeFramebuffer>>,
}

impl ScanoutM1ProbeEntry {
    fn rejected() -> Self {
        Self { _framebuffer: None }
    }

    fn accepted(framebuffer: crate::drm::modeset::DirectScanoutProbeFramebuffer) -> Self {
        Self {
            _framebuffer: Some(std::sync::Arc::new(framebuffer)),
        }
    }

    fn framebuffer(
        &self,
    ) -> Option<&std::sync::Arc<crate::drm::modeset::DirectScanoutProbeFramebuffer>> {
        self._framebuffer.as_ref()
    }
}
```

The accept site (backend.rs:1754) `ScanoutM1ProbeEntry::accepted(framebuffer)` is unchanged (the `Arc::new` is inside `accepted` now).

- [ ] **Step 3: Implement — retain the Arc at the submit block**

At the `try_present_direct` submit block (backend.rs:13216-13224), the fb is looked up as `Option<&Arc<...>>`. **Borrow conflict (second adversarial review, BLOCKING):** do NOT keep `fb_ref` alive across the `&mut self` calls below (`pin_direct_source`, the `pending =` assignment). Rust rejects `&mut self` calls while `fb_ref` still borrows `self.scanout_m1`. Clone to an owned `Arc` immediately after the lookup, then use the owned value throughout:

```rust
let fb = match self
    .scanout_m1
    .entries
    .get(&source_id)
    .and_then(ScanoutM1ProbeEntry::framebuffer)
{
    Some(fb_ref) => std::sync::Arc::clone(fb_ref),
    None => return Ok(false),
};
```

Then in the `DirectPresentFrame` construction (backend.rs:13256):

```rust
self.scanout_m2.pending = Some(DirectPresentFrame {
    source_pin,
    fallback_target_pin,
    source_id,
    candidate,
    fallback_target,
    event,
    awaiting_outputs,
    fb: Some(std::sync::Arc::clone(&fb)),
});
```

And the atomic submit uses `fb.handle()`:

```rust
if let Err(error) = crate::drm::modeset::submit_direct_scanout(
    &self.platform.device,
    fb.handle(),
    &plane_states,
) {
```

> The owned `fb: Arc<...>` local (not a borrow) ends the `&self.scanout_m1` borrow, so the subsequent `&mut self` calls compile. Task 3's queued-store branch and `prepare_direct_frame` use the same pattern — clone to an owned local, pass `&fb`.

- [ ] **Step 4: Single wake-pin registration point**

At the submit block, the wake-pin insert (backend.rs:13250-13254) already keys by `event.present_id`. Keep it there (prepare-time registration for queued frames comes in Task 3). No change in this task beyond confirming the existing insert is the single registration point for the submitted path.

- [ ] **Step 5: Run tests**

Run: `cargo test -p yserver --lib direct_frame_requires_retained_framebuffer_before_queue`
Run: `cargo test -p yserver --lib scanout`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/yserver/src/kms/render/backend.rs
git commit -m "feat(scanout): retain the probe framebuffer by Arc on direct frames"
```

---

## Task 3: Queued-store branch in `try_present_direct`

**Files:**
- Modify: `crates/yserver/src/kms/render/backend.rs:13201-13215` (the gate), `:317-353` (`ScanoutM2State` — add `queued` field + reset), `:1328-1345` (`stop_direct_after_scanout_replaced` — clear `queued`).
- Test: same file, pure predicate + state-transition tests.

**Interfaces:**
- Consumes: `DirectPresentFrame` (with `fb` from Task 2), `ScanoutM2State.pending`/`current`/`unflip_requested`/`hold_direct`/`unflip_fallback_source`/`unflip_shadow_ready`.
- Produces: `ScanoutM2State.queued: Option<DirectPresentFrame>`; `direct_queued_store_eligible(pending_in_flight: bool, scene_flip_in_flight: bool) -> bool`; `complete_queued_as_skip(&mut self)` helper.

- [ ] **Step 1: Add the pure predicate + failing test**

```rust
/// A present may enter the queued latest-wins slot only when a direct flip is
/// in flight AND no scene flip is in flight (the degraded composed-unflip
/// window — both pending — must fall to the existing unflip/Copy path).
fn direct_queued_store_eligible(pending_in_flight: bool, scene_flip_in_flight: bool) -> bool {
    pending_in_flight && !scene_flip_in_flight
}

#[test]
fn direct_queued_store_eligible_gates_on_direct_pending_without_scene_flip() {
    use super::direct_queued_store_eligible as e;
    assert!(e(true, false), "direct pending, scene idle → queue");
    assert!(!e(false, false), "no direct pending → no queue");
    assert!(!e(true, true), "degraded window (both flips) → fall to Copy");
}
```

- [ ] **Step 2: Add the `queued` field to `ScanoutM2State`**

In the struct (backend.rs:317) add `queued: Option<DirectPresentFrame>`, and in `ScanoutM2State::new()` (backend.rs:338) initialize `queued: None`. Update `stop_direct_after_scanout_replaced` (backend.rs:1328) to clear `queued` AND reset `pending_is_submitted` (second adversarial review, IMPORTANT — do not leave a stale `true` from a torn-down direct frame; harmless today but the plan must not contradict its own Task 4 Step 2 instruction):

```rust
if let Some(queued) = self.scanout_m2.queued.take() {
    let mut event = queued.event;
    event.completion_mode = yserver_protocol::x11::present::COMPLETE_MODE_SKIP;
    event.emit_idle = true;
    self.scanout_m2.completed.push(event);
    <Self as Backend>::release_present_source(self, queued.source_pin);
    <Self as Backend>::release_present_source(self, queued.fallback_target_pin);
}
self.scanout_m2.pending_is_submitted = false;
```

**Teardown coverage (spec §2 teardown — enumerate all call sites):** every caller of `stop_direct_after_scanout_replaced` routes through this single function, so the `queued` clear above covers all of them, but VERIFY each at review time: `cargo test`-build the backend and confirm the four call sites compile and route here — VT suspend (`run_suspend`), DPMS off, topology change/RANDR, shutdown (backend.rs:6783, 7447, 7619, 20656 — check the exact labels against the code at each site). Each must release the queued frame's pins and deliver its completion event (the `completed.push` above). Cross-check against each `scanout_m1.clear` call site (backend.rs:1599 topology clear has no preceding `stop_direct` — the `Arc<DirectScanoutProbeFramebuffer>` from Task 2 makes a queued frame safe across that clear, but its completion must still fire via the pending-frame path).

- [ ] **Step 3: Implement the queued-store branch (replace backend.rs:13212-13215)**

The fb lookup stays in the caller (per the Step 4 contract below) — do the lookup inline here, cloning to an owned `Arc` to avoid the borrow conflict with the `&mut self` calls that follow (second adversarial review, BLOCKING):

```rust
if self.scanout_m2.pending.is_some() {
    if direct_queued_store_eligible(
        /* pending_in_flight */ true,
        self.scene.has_pending_page_flips(),
    ) {
        let fb = match self
            .scanout_m1
            .entries
            .get(&source_id)
            .and_then(ScanoutM1ProbeEntry::framebuffer)
        {
            Some(fb_ref) => std::sync::Arc::clone(fb_ref),
            None => {
                // No probe fb for this source: cannot queue a frame that could
                // not flip. Fall to the existing unflip/Copy path.
                self.request_direct_unflip();
                return Ok(false);
            }
        };
        // Latest-wins: a fresh eligible present while the direct flip is in
        // flight replaces any queued frame instead of tearing the direct
        // frame down (the thrash the spec kills).
        if let Some(prev) = self.scanout_m2.queued.take() {
            self.complete_queued_as_skip(prev);
        }
        self.scanout_m2.queued = Some(self.prepare_direct_frame(
            source_id,
            candidate,
            fallback_target,
            event,
            &fb,
        ));
        // Restore the pre-gate state exactly as the submit block does — the
        // unconditional `request_direct_unflip()` above must not leave a stale
        // unflip pending or armed fallback markers.
        self.scanout_m2.unflip_requested = false;
        self.scanout_m2.hold_direct = true;
        self.scanout_m2.unflip_fallback_source = None;
        self.scanout_m2.unflip_shadow_ready = false;
        return Ok(true);
    }
    self.request_direct_unflip();
    return Ok(false);
}
if self.scene.has_pending_page_flips() {
    self.request_direct_unflip();
    return Ok(false);
}
```

- [ ] **Step 4: Factor `prepare_direct_frame` and `complete_queued_as_skip`**

**fb lookup contract (adversarial review I1):** the fb lookup stays in the CALLER (try_present_direct), NOT inside `prepare_direct_frame`. The caller does the `scanout_m1.entries.get(...)` lookup (backend.rs:13216) and passes an owned `Arc<DirectScanoutProbeFramebuffer>` (cloned from the cache) into `prepare_direct_frame`. A cache miss in the caller returns `Ok(false)` (fall to Copy, benign) — exactly the current behavior. `prepare_direct_frame` is infallible (no `?`); the queued-store branch restores `unflip_requested`/`hold_direct` after the prepare call unconditionally.

Extract the pin/wake/fb preparation (backend.rs:13226-13264, minus the atomic submit) into:

```rust
fn prepare_direct_frame(
    &mut self,
    source_id: DrawableId,
    candidate: PresentScanoutCandidate,
    fallback_target: PaintTarget,
    event: yserver_core::backend::CompletedPresentEvent,
    fb: &std::sync::Arc<crate::drm::modeset::DirectScanoutProbeFramebuffer>,
) -> DirectPresentFrame {
    let source_pin = self.pin_direct_source(source_id);
    let fallback_target_pin = self.pin_direct_source(fallback_target.id);
    let wake_pin = self.pin_present_wake_for_direct(&event);
    if !matches!(wake_pin, PinnedWake::None) {
        // SINGLE wake-pin registration point (adversarial review I2): register
        // here, at prepare time, keyed by event.present_id. The chain submit
        // must NOT re-insert.
        self.retained_present_wakes.insert(event.present_id, wake_pin);
    }
    DirectPresentFrame {
        source_pin,
        fallback_target_pin,
        source_id,
        candidate,
        fallback_target,
        event,
        awaiting_outputs: (0..self.platform.outputs.len()).collect(),
        fb: Some(std::sync::Arc::clone(fb)),
    }
}
```

> **No `Result` (second adversarial review, MINOR):** `prepare_direct_frame` cannot fail — `pin_direct_source`, `pin_present_wake_for_direct`, `Arc::clone` are all infallible. Return the frame directly (no `io::Result`), so the queued-store branch calls it without `?` and there is no stale-state-on-`Err` concern:
> `self.scanout_m2.queued = Some(self.prepare_direct_frame(source_id, candidate, fallback_target, event, &fb));`
> The plane states are rebuilt at submit time (chain-flip); they are NOT stored on the frame.

**Submit-failure wake handling (adversarial review I2):** when `submit_direct_scanout` fails after `prepare_direct_frame` registered the wake pin, remove the retained wake (`self.retained_present_wakes.remove(&event.present_id)`) before releasing the pins, so a stale direct wake cannot be signaled for a present the Copy fallback will re-register. The core's Copy fallback re-inserts the same `present_id` via `fire_pending_present_entry` (backend.rs:6431) — with the stale entry removed first, the re-insert is clean.

```rust
fn complete_queued_as_skip(&mut self, frame: DirectPresentFrame) {
    let mut event = frame.event;
    event.completion_mode = yserver_protocol::x11::present::COMPLETE_MODE_SKIP;
    event.emit_idle = true;
    self.scanout_m2.completed.push(event);
    <Self as Backend>::release_present_source(self, frame.source_pin);
    <Self as Backend>::release_present_source(self, frame.fallback_target_pin);
}
```

The production submit block (backend.rs:13250-13264) then becomes: `prepare_direct_frame(...)` (which registers the wake pin), then the atomic submit, and on submit failure remove the retained wake + release pins + reset probation (existing 13244-13247 behavior, plus the wake removal).

- [ ] **Step 5: Run tests**

Run: `cargo test -p yserver --lib direct_queued_store_eligible`
Run: `cargo test -p yserver --lib scanout`
Run: `cargo test -p yserver --lib` (full suite — the submit-block refactor must not break the existing direct path)
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/yserver/src/kms/render/backend.rs
git commit -m "feat(scanout): queue latest-wins direct presents instead of unflipping"
```

---

## Task 4: Chain-flip promotion + `maybe_composite` submit

**Files:**
- Modify: `crates/yserver/src/kms/render/backend.rs:1474-1530` (`retire_direct_output`), `:12900-12945` (`maybe_composite` gate + chain submit), `:317-353` (`ScanoutM2State` — add `pending_is_submitted`).
- Test: same file, pure predicates + state-transition tests.

**Interfaces:**
- Consumes: `ScanoutM2State.queued`, `pending`, `pending_is_submitted: bool`, `unflip_requested`, `hold_direct`, `prepare_direct_frame`/`complete_queued_as_skip` (Task 3).
- Produces: `direct_chain_promote_eligible(queued_some: bool, unflip_requested: bool) -> bool`; `direct_chain_submit_eligible(pending_promoted: bool, scene_flip_in_flight: bool, unflip_requested: bool) -> bool`.

- [ ] **Step 1: Add the pure predicates + failing tests**

```rust
/// At flip retire, promote the queued frame to pending (chain-flip) only when
/// no unflip was requested while it waited (cursor/overlay change → the
/// composed unflip owns the slot instead).
fn direct_chain_promote_eligible(queued_some: bool, unflip_requested: bool) -> bool {
    queued_some && !unflip_requested
}

/// maybe_composite may submit the promoted (not-yet-submitted) pending frame
/// only when no scene flip is in flight and no unflip was requested.
fn direct_chain_submit_eligible(
    pending_promoted: bool,
    scene_flip_in_flight: bool,
    unflip_requested: bool,
) -> bool {
    pending_promoted && !scene_flip_in_flight && !unflip_requested
}

#[test]
fn direct_chain_promote_eligible_gates_on_no_unflip() {
    use super::direct_chain_promote_eligible as e;
    assert!(e(true, false));
    assert!(!e(true, true), "unflip requested → queued is skipped, not chained");
    assert!(!e(false, false));
}

#[test]
fn direct_chain_submit_eligible_gates_on_scene_flip_and_unflip() {
    use super::direct_chain_submit_eligible as e;
    assert!(e(true, false, false));
    assert!(!e(true, true, false), "degraded window: scene flips in flight");
    assert!(!e(true, false, true), "unflip requested → composed unflip");
    assert!(!e(false, false, false), "nothing promoted");
}
```

- [ ] **Step 2: Add `pending_is_submitted` to `ScanoutM2State`**

Struct + `new()` initializer. Set `true` at every direct submit (both the initial submit block and, in Task 4 Step 4, the chain submit). Set `false` when a frame is promoted (Step 3) and on any `request_direct_unflip`/teardown that clears the slot.

- [ ] **Step 3: Implement the promotion + skip in `retire_direct_output`**

Rewrite the **whole** `pending`-handling path in `retire_direct_output` (backend.rs:1496-1528) as ONE block — do NOT "fall through" to the existing code, because the existing code does `pending.take()` which would grab the promoted frame instead of the retired one (adversarial review B2). The retired frame must be captured BEFORE promoting. **The phantom-retire guard goes BEFORE the `remove(&output_idx)`** (third adversarial review, BLOCKING): the guard was previously placed inside the `is_empty` branch, which fires only AFTER `pending.awaiting_outputs.remove()` already mutated the set — so (a) on multi-output a scene retire was absorbed into the promoted-unsubmitted frame's awaiting set (returned `true`, `scene.has_pending_page_flips()` stayed pinned, the chain submit starved), and (b) on single-output the `remove` emptied the awaiting set of a frame that never went on the wire, so its own real retire later found nothing to consume and the direct frame hung forever. A promoted-unsubmitted frame is NOT on the KMS pipe — no flip retire can be for it, so it must be rejected BEFORE consuming the output:

```rust
// Guard comes before `awaiting_outputs.remove` (third adversarial review,
// BLOCKING): only KMS-submitted frames may consume retires. A
// promoted-unsubmitted frame sits in `pending` but is NOT on the pipe;
// `on_page_flip_ready` calls `retire_direct_output` before
// `scene.handle_page_flip_complete` (backend.rs:12680-12687), so a scene
// flip in flight in that window (the degraded composed-unflip case) would
// otherwise have its retire absorbed into the promoted frame's awaiting
// set — pinning `scene.has_pending_page_flips()` true, starving the chain
// submit, and (on single-output) emptying the set of a frame that never
// went on the wire so its own retire can never fire. Hand scene retires
// back without mutating the frame.
let Some(pending) = self.scanout_m2.pending.as_mut() else {
    return false;
};
if !self.scanout_m2.pending_is_submitted {
    return false;
}
if !pending.awaiting_outputs.remove(&output_idx) {
    return false;
}
if pending.awaiting_outputs.is_empty() {
    // 1. Capture the RETIRED frame (its flip just completed) — before any
    //    promotion replaces the `pending` slot.
    let mut retired = self
        .scanout_m2
        .pending
        .take()
        .expect("pending direct frame disappeared");
    retired.event.completion_mode = yserver_protocol::x11::present::COMPLETE_MODE_FLIP;
    retired.event.emit_idle = false;

    // 2. Chain-flip promotion: if a queued frame waited and no unflip was
    //    requested, promote it to pending (submitted next tick by
    //    maybe_composite). If an unflip WAS requested, skip the queued frame.
    if direct_chain_promote_eligible(
        self.scanout_m2.queued.is_some(),
        self.scanout_m2.unflip_requested,
    ) {
        let promoted = self.scanout_m2.queued.take().expect("checked Some");
        self.scanout_m2.pending = Some(promoted);
        self.scanout_m2.pending_is_submitted = false;
        self.scanout_m2.hold_direct = false; // chain submit gate (B1)
        log::info!("scanout_m2: chain-flip promoted source_id={}", promoted.source_id.as_u64());
    } else if let Some(queued) = self.scanout_m2.queued.take() {
        self.complete_queued_as_skip(queued);
    }

    // 3. Deliver the RETIRED frame's FLIP completion and make it current.
    self.scanout_m2.completed.push(retired.event.clone());
    if let Some(previous) = self.scanout_m2.current.replace(retired) {
        self.release_direct_frame(previous);
    }
    log::info!(
        "scanout_m2: direct frame retired on all outputs source_id={}",
        self.scanout_m2
            .current
            .as_ref()
            .map_or(0, |frame| frame.source_id.as_u64())
    );
    self.bind_direct_cursor_on_all_outputs();
}
```

> **Borrow note (third adversarial review):** the `pending: &mut` borrow of `self.scanout_m2.pending` coexists with reading `self.scanout_m2.pending_is_submitted` — these are disjoint fields of `ScanoutM2State`, so the borrow checker accepts it. Then `self.scanout_m2.pending.take()` inside `is_empty` is fine once the `pending` borrow ends after `is_empty()` (NLL). If the compiler still balks, restructure to check the guard first and read the awaiting set through `self.scanout_m2.pending.as_ref()`.
>
> **Add a regression test (third adversarial review, BLOCKING):** the promotion tests in Step 5 must also pin the guard — a scene retire hitting a promoted-unsubmitted frame must NOT consume its output and must hand the retire back:
> `b.scanout_m2.pending = Some(promoted_frame); b.scanout_m2.pending_is_submitted = false; assert!(!b.retire_direct_output(0), "phantom retire handed back"); assert!(b.scanout_m2.pending.as_ref().unwrap().awaiting_outputs.contains(&0), "awaiting set untouched");` — then flip `pending_is_submitted = true` and assert the same call now consumes output 0 and (when the set empties) retires normally.

> The `else if let Some(queued)` skip path MUST also handle the case where `pending` was already empty when an unflip is requested — but at this branch `pending` is guaranteed `Some` (we just `take()`d it above, so `pending` may be `None` only if it started empty; the existing `expect` covers that). If `unflip_requested` is true and there is no `queued`, nothing to skip — the `else if let` handles it.
>
> Note the promotion sets `hold_direct = false` (adversarial review B1): `maybe_composite`'s gate conjunct 3 is `(hold_direct && !unflip_requested)`, which would otherwise early-return before the chain submit.

- [ ] **Step 4: Amend the `maybe_composite` gate + add the chain submit**

The gate at backend.rs:12911-12914 must admit the promoted-unsubmitted frame. Change conjunct 1 to require the pending frame be actually in flight, AND change conjunct 3 so `hold_direct` no longer blocks a promoted frame (adversarial review B1):

```rust
if (self.scanout_m2.pending.is_some() && self.scanout_m2.pending_is_submitted)
    || !self.scanout_m2.unflip_awaiting_outputs.is_empty()
    || (self.scanout_m2.hold_direct
        && !self.scanout_m2.unflip_requested
        && (self.scanout_m2.pending.is_none()
            || self.scanout_m2.pending_is_submitted))
{
    self.drain_render_telemetry();
    self.telemetry.maybe_emit(self.engine.pending_count());
    return Ok(());
}
```

Then, before the composed-unflip branch, add the chain submit. The failure path MUST complete the promoted frame's event and release its pins (adversarial review B6) — `submit_chain_direct_frame` returns the frame on failure so the caller can do that:

```rust
if self.scanout_m2.pending.is_some()
    && direct_chain_submit_eligible(
        /* pending_promoted */ !self.scanout_m2.pending_is_submitted,
        self.scene.has_pending_page_flips(),
        self.scanout_m2.unflip_requested,
    )
{
    match self.submit_chain_direct_frame() {
        Ok(()) => {
            self.scanout_m2.pending_is_submitted = true;
            self.scanout_m2.hold_direct = true;
            self.scanout_m2.unflip_requested = false;
            self.drain_render_telemetry();
            self.telemetry.maybe_emit(self.engine.pending_count());
            return Ok(());
        }
        Err((error, failed)) => {
            log::error!("scanout_m2: chain direct submit failed: {error}; composed fallback");
            // Failure contract (spec): release pins, complete as Skip/Copy,
            // reset probation, reentry-blocked, then ARM the composed unflip
            // so `current`'s pins are released when the composed fallback
            // retires. WITHOUT this (adversarial review, IMPORTANT): the chain
            // submit ran only because `unflip_requested` was false, so the
            // composed-unflip branch below (backend.rs:12919) would be skipped,
            // the scene flips replacing the direct planes would retire through
            // the scene, `stop_direct_after_scanout_replaced` would never run,
            // and `current`'s pins would stay held until a future direct
            // re-entry replaced them (a stopped flood holds them indefinitely).
            self.scanout_m2.pending = None;
            self.scanout_m2.pending_is_submitted = false;
            self.complete_queued_as_skip(failed);
            self.scanout_m2.reset_eligible_root_probation();
            self.scanout_m2.reentry_blocked_until_composed = true;
            // `active()` is true (`current` is Some — the retired frame is
            // still scanned), so this arms `unflip_requested` + clears
            // `hold_direct`, which makes the composed-unflip branch right below
            // run `submit_composed_unflip` this tick.
            self.request_direct_unflip();
            self.scene.mark_scene_structure_dirty();
            // fall through to the composed-unflip branch below — do NOT return
            // early. On its failure that branch degrades to per-output composed
            // flips (backend.rs:12932-12935), whose retires release `current`
            // via `stop_direct_after_scanout_replaced`; on its success the
            // atomic replacement retires through the unflip path the same way.
        }
    }
}
```

`submit_chain_direct_frame` (mirrors the direct submit):

```rust
fn submit_chain_direct_frame(
    &mut self,
) -> Result<(), (io::Error, DirectPresentFrame)> {
    let frame = self
        .scanout_m2
        .pending
        .take()
        .expect("chain submit called with a promoted pending frame");
    let fb = frame
        .fb
        .as_ref()
        .expect("promoted frame retains its probe framebuffer");
    let plane_states: Vec<crate::drm::modeset::DirectScanoutPlaneState<'_>> = self
        .platform
        .outputs
        .iter()
        .map(|layout| crate::drm::modeset::DirectScanoutPlaneState {
            output: &layout.output,
            src_x: u32::try_from(layout.x).expect("M1 validated non-negative x"),
            src_y: u32::try_from(layout.y).expect("M1 validated non-negative y"),
            src_w: u32::from(layout.width),
            src_h: u32::from(layout.height),
        })
        .collect();
    if let Err(error) = crate::drm::modeset::submit_direct_scanout(
        &self.platform.device,
        fb.handle(),
        &plane_states,
    ) {
        return Err((error, frame)); // caller completes+releases it
    }
    self.scanout_m2.pending = Some(frame);
    log::info!(
        "scanout_m2: chain direct submit source_id={} present_id={} outputs={}",
        self.scanout_m2.pending.as_ref().map_or(0, |f| f.source_id.as_u64()),
        self.scanout_m2.pending.as_ref().map_or(0, |f| f.event.present_id),
        self.platform.outputs.len()
    );
    Ok(())
}
```

> The failure path calls `complete_queued_as_skip(failed)` which releases both pins AND pushes the SKIP event — so the client's present completes and its buffers are released. This satisfies the spec's chain-fail contract (B6).

- [ ] **Step 5: State-transition tests**

Add a test that exercises the promotion hand-off. The fixture CAN drive `retire_direct_output` if the frame's `awaiting_outputs` is `{0}` and `current` is `None` (the `current.replace` old-value path is skipped, and `bind_direct_cursor_on_all_outputs` is safe with the stub platform's 0 outputs). Give the retired and promoted frames distinct `present_id`s so the assertions are meaningful (adversarial review I3 — the test must ASSERT, not just comment):

```rust
#[test]
fn retire_promotes_queued_and_marks_pending_unsubmitted() {
    let mut b = super::KmsBackend::for_tests();
    // Retired frame (in flight) with a distinct present_id.
    let mut retired = super::DirectPresentFrame::for_tests();
    retired.event.present_id = 10;
    retired.awaiting_outputs.insert(0);
    // Queued frame (next) with a distinct present_id.
    let mut queued = super::DirectPresentFrame::for_tests();
    queued.event.present_id = 20;
    b.scanout_m2.pending = Some(retired);
    b.scanout_m2.pending_is_submitted = true;
    b.scanout_m2.queued = Some(queued);
    b.scanout_m2.unflip_requested = false;
    b.scanout_m2.cursor_bound_all = true; // fixture has 1 output + no cursor plane; short-circuit bind

    let handled = b.retire_direct_output(0);
    assert!(handled, "retire of the sole output must be handled");
    assert!(b.scanout_m2.pending.is_some(), "promoted frame becomes pending");
    assert_eq!(
        b.scanout_m2.pending.as_ref().map(|f| f.event.present_id),
        Some(20),
        "pending holds the promoted queued frame, not the retired one"
    );
    assert!(!b.scanout_m2.pending_is_submitted, "promoted frame not yet submitted");
    assert!(b.scanout_m2.queued.is_none(), "queued slot emptied by promotion");
    assert!(
        b.scanout_m2.current.as_ref().map(|f| f.event.present_id) == Some(10),
        "current holds the retired frame with its FLIP completion"
    );
    assert_eq!(b.scanout_m2.completed.len(), 1, "retired FLIP completion delivered once");
    assert_eq!(
        b.scanout_m2.completed[0].present_id, 10,
        "the delivered completion is the retired frame, not the promoted one"
    );
    assert!(!b.scanout_m2.hold_direct, "promotion clears hold_direct for the chain gate (B1)");
}

#[test]
fn retire_skips_queued_when_unflip_requested() {
    let mut b = super::KmsBackend::for_tests();
    let mut retired = super::DirectPresentFrame::for_tests();
    retired.event.present_id = 10;
    retired.awaiting_outputs.insert(0);
    let mut queued = super::DirectPresentFrame::for_tests();
    queued.event.present_id = 20;
    b.scanout_m2.pending = Some(retired);
    b.scanout_m2.pending_is_submitted = true;
    b.scanout_m2.queued = Some(queued);
    b.scanout_m2.unflip_requested = true;
    b.scanout_m2.cursor_bound_all = true; // fixture has 1 output + no cursor plane; short-circuit bind

    b.retire_direct_output(0);
    assert!(b.scanout_m2.pending.is_none(), "unflip requested → no promotion");
    assert!(b.scanout_m2.queued.is_none(), "queued skipped on unflip");
    // complete_queued_as_skip pushes the SKIP FIRST (backend.rs:583 of the
    // plan), then the retired FLIP is pushed (plan:587). So completed[0] is
    // the queued SKIP (present_id 20), completed[1] is the retired FLIP
    // (present_id 10). Do NOT assert present_id 0 — the fixture frame's
    // present_id is 1 at default, and here we set both explicitly.
    assert_eq!(b.scanout_m2.completed.len(), 2, "retired FLIP + queued SKIP both delivered");
    assert_eq!(b.scanout_m2.completed[0].present_id, 20, "queued SKIP first");
    assert_eq!(b.scanout_m2.completed[0].completion_mode, yserver_protocol::x11::present::COMPLETE_MODE_SKIP);
    assert_eq!(b.scanout_m2.completed[1].present_id, 10, "retired FLIP second");
    assert_eq!(b.scanout_m2.completed[1].completion_mode, yserver_protocol::x11::present::COMPLETE_MODE_FLIP);
}

#[test]
fn phantom_retire_rejected_before_promoted_frame_is_submitted() {
    let mut b = super::KmsBackend::for_tests();
    // A promoted-unsubmitted frame sits in `pending` (never on the KMS pipe).
    let mut promoted = super::DirectPresentFrame::for_tests();
    promoted.event.present_id = 20;
    promoted.awaiting_outputs.insert(0);
    b.scanout_m2.pending = Some(promoted);
    b.scanout_m2.pending_is_submitted = false;
    b.scanout_m2.cursor_bound_all = true; // fixture has 1 output + no cursor plane; short-circuit bind

    // A scene flip retires in that window (the degraded composed-unflip
    // case). The guard must hand the retire back WITHOUT consuming the
    // promoted frame's output.
    assert!(
        !b.retire_direct_output(0),
        "phantom retire on a promoted-unsubmitted frame is handed to the scene"
    );
    assert!(
        b.scanout_m2.pending.as_ref().unwrap().awaiting_outputs.contains(&0),
        "the promoted frame's awaiting set is untouched by a phantom retire"
    );

    // Once the chain submit actually puts the frame on the wire, the SAME
    // retire call legitimately consumes the output.
    b.scanout_m2.pending_is_submitted = true;
    assert!(b.retire_direct_output(0), "real retire consumed after submit");
    assert!(b.scanout_m2.pending.is_none(), "frame retired normally");
}
```

> **Fixture output count (second adversarial review, MINOR):** `PlatformBackend::for_tests()` builds `outputs: vec![OutputLayout{...}]` (platform.rs:1044) — **1 output**, not 0. `bind_direct_cursor_on_all_outputs` (called at the end of the retire branch) iterates output 0, finds `cursor_plane: None`, and calls `request_direct_unflip()` (backend.rs:1282-1283) — so every `retire_direct_output(0)` flips `unflip_requested` to `true` as a side effect. No panic, tests still pass (they don't assert `unflip_requested` after retire), but the premise is wrong. Pre-set `b.scanout_m2.cursor_bound_all = true;` in both tests before calling `retire_direct_output(0)` so `bind_direct_cursor_on_all_outputs` short-circuits, or drop the "0 outputs" claim. Recommended: set `cursor_bound_all = true` in both tests for determinism.

- [ ] **Step 6: Run tests**

Run: `cargo test -p yserver --lib direct_chain`
Run: `cargo test -p yserver --lib retire_promotes_queued`
Run: `cargo test -p yserver --lib scanout`
Run: `cargo test -p yserver --lib`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add crates/yserver/src/kms/render/backend.rs
git commit -m "feat(scanout): chain-flip the queued direct frame on the next compose tick"
```

---

## Task 5: yserver-core Skip-ordering test for queued victims

**Files:**
- Test: `crates/yserver-core/src/core_loop/process_request.rs` (test module) — a regression that a queued victim's Skip respects per-window `present_id` order for synced entries, reusing the existing ordered-delivery machinery. No production code change.

**Interfaces:**
- Consumes: the existing hold-back machinery (`present_pending_complete` with a target gate, the per-window `present_id` hold-back in the completion sweep), `present_pending_entry_with` helper (~40502), the existing ordered-delivery tests as templates (`due_skip_is_held_back_behind_a_smaller_id_undrained_gate_entry` ~39781, `due_skip_is_held_back_behind_a_smaller_id_unexecuted_store_entry` ~39884).
- Produces: a regression test pinning that a SKIP completion with `effective_target_msc = 0` (the queued-victim shape, matching the Phase A spec's `Skip eff=0` rule) is held back until the same window's smaller-`present_id` FLIP/COPY entry has drained — the spec §ordering guarantee (spec:226-228).

- [ ] **Step 1: Write the failing test**

The queued-victim Skip is a synced entry delivered via `present_pending_complete` with `effective_target_msc = 0` (immediate gate). The test builds a window with two pending entries: a smaller-`present_id` unexecuted store entry (the in-flight FLIP) and a larger-`present_id` Skip at eff=0. Assert the Skip does NOT drain ahead of the smaller id. Model it on `due_skip_is_held_back_behind_a_smaller_id_unexecuted_store_entry` (~39884): the drain entry point is `fire_due_present_completions(&mut state, &mut backend, clock)` (NOT a nonexistent `drain_ordered_present_completions`), and the assertion shape is `backend.signalled_present_wakes.is_empty()`. The clock's `msc` must be nonzero (the sweep early-returns on `clock.msc == 0`) — any target works since the hold-back fires on `present_id` order regardless of the Skip's eff=0. Use `present_pending_entry_with` for the entries:

```rust
#[test]
fn queued_victim_skip_held_back_behind_smaller_id_unexecuted_store_entry() {
    use yserver_protocol::x11::present as x11present;

    // Spec 2026-08-12-direct-scanout-latest-wins-supersession §ordering:
    // a queued direct victim's Skip (synced, effective_target_msc = 0) must
    // not overtake a same-window smaller-present_id entry still in the
    // unexecuted store — the chain-flip FLIP retires in present_id order.
    let mut state = ServerState::new();
    let mut backend = RecordingBackend::new();
    const WINDOW: u32 = 0x00e0_3001;
    let clock = crate::backend::PresentClockSample {
        msc: 50,
        ust: 0x2000,
        source: crate::backend::PresentClockSource::PageFlip,
    };

    // Smaller present_id: an unexecuted store entry (the in-flight direct
    // FLIP, synced target).
    let flip_entry = present_pending_entry_with(1, WINDOW, 0x00e0_3002, Some(100), true);
    state.present_pending_exec.insert(1, flip_entry);

    // Larger present_id: the queued victim's Skip, gated at 0 (immediate).
    let skip_entry = present_pending_entry_with(2, WINDOW, 0x00e0_3003, Some(0), true);
    state.present_pending_complete.push(crate::server::PendingPresentComplete {
        event: completed_event_for_pending(&skip_entry.pending),
        effective_target_msc: 0,
        mode: x11present::COMPLETE_MODE_SKIP,
        emit_idle: true,
    });

    // Drain: the sweep must hold the Skip back behind the smaller id's
    // unexecuted store entry.
    fire_due_present_completions(&mut state, &mut backend, clock);
    assert!(
        backend.signalled_present_wakes.is_empty(),
        "the queued-victim Skip must not drain ahead of the smaller-present_id \
         unexecuted FLIP entry (spec §ordering)"
    );
    assert_eq!(
        state.present_pending_complete.len(),
        1,
        "the held-back Skip survives the sweep, still parked in present_id order"
    );
}
```

> The existing tests at ~39781-39958 are the templates — copy their drain call (`fire_due_present_completions(&mut state, &mut backend, clock)`) and their assertion shape (`backend.signalled_present_wakes.is_empty()`) verbatim; do not invent a `drain_ordered_present_completions` helper. `fire_due_present_completions` (process_request.rs:10810) and `completed_event_for_pending` (process_request.rs:8704) are both in scope via the test module's `use super::*`. The second assertion (`present_pending_complete.len() == 1`) confirms the entry was not dropped but parked.

- [ ] **Step 2: Run it, expect PASS (the hold-back already exists for synced entries)**

Run: `cargo test -p yserver-core --lib queued_victim_skip`
Expected: PASS — this is a regression PIN, not a failing-first test; the ordered-delivery machinery already enforces the hold-back. If it FAILS, the spec's ordering guarantee is already broken — investigate before proceeding (do not just adjust the test).

- [ ] **Step 3: Commit**

```bash
git add crates/yserver-core/src/core_loop/process_request.rs
git commit -m "test(present): pin queued-victim Skip ordering behind smaller present ids"
```

---

## Task 6: Hardware validation on the nvidia box

**Files:**
- Create: append result to the spec's acceptance tracking (the findings doc `docs/superpowers/findings/2026-08-11-cs2-fullscreen-novsync-pageflip-collapse.md` §6, or a new findings section).

- [ ] **Step 1: Rebuild release and launch the telemetry session**

```bash
cargo build --release --bin yserver
# from tty2, with YSERVER_HW_CURSOR_NVIDIA=1 + telemetry:
YSERVER_LOOP_TELEMETRY=1 YSERVER_SUBMIT_TRACE=yserver-cinnamon.submit.tsv \
  YSERVER_HW_CURSOR_NVIDIA=1 RUST_LOG=info RUST_BACKTRACE=1 \
  target/release/yserver > yserver-hw-cinnamon.log 2>&1 &
# + dbus-run-session cinnamon-session (DISPLAY=:7), or `just yserver-cinnamon-hw-telemetry`
```

- [ ] **Step 2: Play CS2 fullscreen vsync OFF ~3 min**

- [ ] **Step 3: Verify the acceptance metrics**

Run:
```bash
grep "loop telemetry" yserver-hw-cinnamon.log | grep -o "page_flip/s=[0-9.]*" | tail -40
# Expected: ~60 sustained (was 54-56)
grep -c "scanout_m2: composed unflip retired" yserver-hw-cinnamon.log
# Expected: near 0 in steady state (was 553/session)
grep -c "scanout_m2: live direct submit" yserver-hw-cinnamon.log
# Expected: > 0 (direct scanout still engaged)
grep -i "request_exit" yserver-hw-cinnamon.log
# Expected: nothing
grep -c "chain direct submit failed" yserver-hw-cinnamon.log
# Expected: 0
```

- [ ] **Step 4: Check for the queued-slot Skip path**

Run: `grep -c "queued.*skip\|complete_queued_as_skip\|stage=queued_skip" yserver-hw-cinnamon.log`
Expected: small/non-zero if the slot exercised; 0 is acceptable if Piece 1 coalesces so well the slot never fills.

- [ ] **Step 5: Record results in the findings doc, commit**

```bash
git add docs/superpowers/findings/2026-08-11-cs2-fullscreen-novsync-pageflip-collapse.md
git commit -m "docs(scanout): record direct-level supersession hardware validation"
```

**Known blockers this task may reveal (do NOT silently skip):**
- If `page_flip/s` still drops below ~58, the core parking (Piece 1) is not coalescing — check `present_skips/s` in the render telemetry: if it is ~0, the synced supersession successor gate is not firing for the game's update region; record it (a follow-up to the successor-gate relaxation, out of scope here).
- If composed unflips are still high, the queued-store branch is not engaging — check `direct_queued_store_eligible` never fires because `scene.has_pending_page_flips()` is true during direct (would contradict Piece 1's premise; record it).
- If the game shows recycled/garbage frames after chain-flips engage, the queued frame's release syncobj is not signaled after the chain flip retires — record it; do not claim success.

---

## Task 7: CI gate + branch finish

- [ ] **Step 1: Clippy exactly as CI**

Run: `cargo clippy --all-targets -- -D warnings`
Expected: clean.

- [ ] **Step 2: Format**

Run: `cargo +nightly fmt`

- [ ] **Step 3: Full test suites**

Run: `cargo test -p yserver --lib && cargo test -p yserver-core --lib`
Expected: all pass.

- [ ] **Step 4: Ask the user for confirmation to squash-merge** (AGENTS.md)

```bash
git log origin/master..HEAD --oneline
```
