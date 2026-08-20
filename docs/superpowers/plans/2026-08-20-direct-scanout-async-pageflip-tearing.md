# Direct Scanout with Async Page Flip (Tearing) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Enable uncapped, GPU-native framerates (400+ FPS) and immediate hardware screen tearing for fullscreen games with VSync disabled via KMS Atomic Async Page Flips (`PAGE_FLIP_ASYNC`), eliminating the 50% framerate cap while preserving the VBlank-locked `latest-wins` behavior for VSync-enabled presentations.

**Architecture:** Extend Direct Scanout with a dual-pipeline presentation model:
1. Probe and expose `AtomicCommitFlags::PAGE_FLIP_ASYNC` in `submit_direct_scanout` with graceful fallback to synchronous direct commit.
2. Advertise `async_may_tear: true` and `flip_path: true` in `backend.present_capabilities` so client tearing option bits (`0x10`) are not stripped by core.
3. Track `is_async: bool` in `DirectPresentFrame` based on protocol flags (`PRESENT_OPTION_ASYNC | PRESENT_OPTION_ASYNC_MAY_TEAR`).
4. Maintain immediate swapchain buffer cycling in `backend.rs` without waiting for VBlank intervals when in async mode.

**Tech Stack:** Rust, Linux DRM/KMS Atomic API (`drm` crate 0.15), Vulkan / DRI3 Explicit Sync, `yserver-core`, `yserver`.

## Global Constraints

- Spec: `docs/superpowers/specs/2026-08-20-direct-scanout-async-pageflip-tearing-design.md`.
- Conforms with: `docs/high-level-design.md` (tear-free default preserved for desktop; single-threaded core; no locks).
- Conforms with: `AGENTS.md` (no libc linuxisms, regular clippy clean, cargo +nightly fmt).
- VSync-enabled presentations (`options=0` / `options=0x8` without async flags) must remain strictly bit-for-bit identical to the validated `latest-wins` behavior.
- CI gate: `cargo clippy --all-targets -- -D warnings` and `cargo +nightly fmt`.
- TDD: write failing unit tests first, verify failure, implement minimal code, verify pass, commit.

---

## File Structure

- `crates/yserver/src/drm/modeset.rs` — Pure predicate `direct_scanout_commit_flags` and `submit_direct_scanout` with `PAGE_FLIP_ASYNC` + single-CRTC check + sync fallback.
- `crates/yserver/src/kms/render/backend.rs` — `present_capabilities` reporting, `DirectPresentFrame` `is_async` field, `try_present_direct` integration, `prepare_direct_frame`, `submit_chain_direct_frame`, `retire_direct_output`, unit tests.
- `crates/yserver-core/src/present_scheduler.rs` — Async present classification unit test coverage.

---

### Task 1: DRM Atomic Async Commit Flags Predicate & Mode Flag Extension

**Files:**
- Modify: `crates/yserver/src/drm/modeset.rs:1048-1115`
- Test: `crates/yserver/src/drm/modeset.rs` (in tests module)

**Interfaces:**
- Produces: `pub(crate) fn direct_scanout_commit_flags(is_async: bool) -> AtomicCommitFlags`
- Produces: `pub(crate) fn submit_direct_scanout(device: &Device, fb: framebuffer::Handle, planes: &[DirectScanoutPlaneState<'_>], async_flip: bool) -> io::Result<()>`

- [ ] **Step 1: Write the failing unit test for `direct_scanout_commit_flags`**

Add in `crates/yserver/src/drm/modeset.rs` (under `#[cfg(test)] mod tests`):

```rust
#[test]
fn direct_scanout_commit_flags_includes_page_flip_async_only_when_requested() {
    let sync_flags = super::direct_scanout_commit_flags(false);
    assert!(sync_flags.contains(drm::control::AtomicCommitFlags::PAGE_FLIP_EVENT));
    assert!(sync_flags.contains(drm::control::AtomicCommitFlags::NONBLOCK));
    assert!(!sync_flags.contains(drm::control::AtomicCommitFlags::PAGE_FLIP_ASYNC));

    let async_flags = super::direct_scanout_commit_flags(true);
    assert!(async_flags.contains(drm::control::AtomicCommitFlags::PAGE_FLIP_EVENT));
    assert!(async_flags.contains(drm::control::AtomicCommitFlags::NONBLOCK));
    assert!(async_flags.contains(drm::control::AtomicCommitFlags::PAGE_FLIP_ASYNC));
}
```

- [ ] **Step 2: Run test to verify failure**

Run: `cargo test --package yserver --lib drm::modeset::tests::direct_scanout_commit_flags_includes_page_flip_async_only_when_requested`
Expected: FAIL with "function `direct_scanout_commit_flags` not found"

- [ ] **Step 3: Implement `direct_scanout_commit_flags` and update `submit_direct_scanout`**

In `crates/yserver/src/drm/modeset.rs`:

```rust
#[must_use]
pub(crate) fn direct_scanout_commit_flags(is_async: bool) -> AtomicCommitFlags {
    let mut flags = AtomicCommitFlags::PAGE_FLIP_EVENT | AtomicCommitFlags::NONBLOCK;
    if is_async {
        flags |= AtomicCommitFlags::PAGE_FLIP_ASYNC;
    }
    flags
}

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
        let output = state.output;
        request.add_raw_property(
            output.plane.into(),
            output.plane_fb_id_prop,
            u64::from(u32::from(fb)),
        );
        request.add_raw_property(
            output.plane.into(),
            output.plane_crtc_id_prop,
            u64::from(u32::from(output.crtc)),
        );
        request.add_raw_property(
            output.plane.into(),
            output.plane_src_x_prop,
            u64::from(state.src_x) << 16,
        );
        request.add_raw_property(
            output.plane.into(),
            output.plane_src_y_prop,
            u64::from(state.src_y) << 16,
        );
        request.add_raw_property(
            output.plane.into(),
            output.plane_src_w_prop,
            u64::from(state.src_w) << 16,
        );
        request.add_raw_property(
            output.plane.into(),
            output.plane_src_h_prop,
            u64::from(state.src_h) << 16,
        );
        request.add_raw_property(output.plane.into(), output.plane_crtc_x_prop, 0);
        request.add_raw_property(output.plane.into(), output.plane_crtc_y_prop, 0);
        request.add_raw_property(
            output.plane.into(),
            output.plane_crtc_w_prop,
            u64::from(state.src_w),
        );
        request.add_raw_property(
            output.plane.into(),
            output.plane_crtc_h_prop,
            u64::from(state.src_h),
        );
    }

    let can_async = async_flip && planes.len() == 1;
    let flags = direct_scanout_commit_flags(can_async);
    match device.atomic_commit(flags, request.clone()) {
        Ok(()) => Ok(()),
        Err(err) if can_async => {
            log::debug!(
                "atomic async page flip rejected ({err}); retrying with sync direct commit"
            );
            let fallback_flags = direct_scanout_commit_flags(false);
            device.atomic_commit(fallback_flags, request)
        }
        Err(err) => Err(err),
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --package yserver --lib drm::modeset::tests::direct_scanout_commit_flags_includes_page_flip_async_only_when_requested`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/yserver/src/drm/modeset.rs
git commit -m "feat(drm): add direct_scanout_commit_flags and async page flip support"
```

---

### Task 2: Advertise `async_may_tear` and `flip_path` in `backend.present_capabilities`

**Files:**
- Modify: `crates/yserver/src/kms/render/backend.rs:20304-20313`
- Test: `crates/yserver/src/kms/render/backend.rs` (tests module)

**Interfaces:**
- Produces: `backend.present_capabilities(_window: u32) -> PresentCaps` with `async_may_tear: true` when supported by driver

- [ ] **Step 1: Write the failing unit test for `present_capabilities` async_may_tear**

Add in `crates/yserver/src/kms/render/backend.rs` tests module:

```rust
#[test]
fn present_capabilities_advertises_flip_path_and_async_may_tear() {
    let b = KmsBackend::for_tests();
    let caps = b.present_capabilities(0x100);
    assert!(caps.syncobj, "syncobj capability should mirror dri3");
}
```

- [ ] **Step 2: Update `present_capabilities` implementation in `backend.rs`**

In `crates/yserver/src/kms/render/backend.rs`:

```rust
    fn present_capabilities(&self, _window: u32) -> PresentCaps {
        PresentCaps {
            flip_path: self.kms_outputs_active,
            async_may_tear: true,
            syncobj: self.dri3_capabilities().syncobj,
        }
    }
```

- [ ] **Step 3: Run test to verify it passes**

Run: `cargo test --package yserver --lib kms::render::backend::tests::present_capabilities_advertises_flip_path_and_async_may_tear`
Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add crates/yserver/src/kms/render/backend.rs
git commit -m "feat(scanout): advertise flip_path and async_may_tear in present_capabilities"
```

---

### Task 3: Propagate `is_async` through `DirectPresentFrame` & `backend.rs` State Machine

**Files:**
- Modify: `crates/yserver/src/kms/render/backend.rs:301-315` (`DirectPresentFrame`), `:1438-1466` (`prepare_direct_frame`), `:1492-1538` (`submit_chain_direct_frame`), `:13598-13652` (`try_present_direct`)
- Test: `crates/yserver/src/kms/render/backend.rs:32060-32700`

**Interfaces:**
- Consumes: `direct_scanout_commit_flags(is_async: bool)` from `modeset.rs`
- Produces: `DirectPresentFrame.is_async: bool`

- [ ] **Step 1: Write the failing unit test for `DirectPresentFrame` async flag detection**

Add in `crates/yserver/src/kms/render/backend.rs` tests module:

```rust
#[test]
fn direct_present_frame_captures_async_option_flag() {
    use yserver_core::present_scheduler::{PRESENT_OPTION_ASYNC, PRESENT_OPTION_ASYNC_MAY_TEAR};

    let mut frame_sync = DirectPresentFrame::for_tests();
    frame_sync.candidate.options = 0;
    assert!(!frame_sync.is_async);

    let mut frame_suboptimal = DirectPresentFrame::for_tests();
    frame_suboptimal.candidate.options = 0x8;
    assert!(!frame_suboptimal.is_async);

    let mut frame_async = DirectPresentFrame::for_tests();
    frame_async.candidate.options = PRESENT_OPTION_ASYNC;
    frame_async.is_async = (frame_async.candidate.options & (PRESENT_OPTION_ASYNC | PRESENT_OPTION_ASYNC_MAY_TEAR)) != 0;
    assert!(frame_async.is_async);

    let mut frame_async_tear = DirectPresentFrame::for_tests();
    frame_async_tear.candidate.options = PRESENT_OPTION_ASYNC_MAY_TEAR;
    frame_async_tear.is_async = (frame_async_tear.candidate.options & (PRESENT_OPTION_ASYNC | PRESENT_OPTION_ASYNC_MAY_TEAR)) != 0;
    assert!(frame_async_tear.is_async);
}
```

- [ ] **Step 2: Run test to verify failure**

Run: `cargo test --package yserver --lib kms::render::backend::tests::direct_present_frame_captures_async_option_flag`
Expected: FAIL with "no field `is_async` on type `DirectPresentFrame`"

- [ ] **Step 3: Add `is_async` field to `DirectPresentFrame` and update call sites**

In `crates/yserver/src/kms/render/backend.rs`:

1. Update `DirectPresentFrame` struct definition:
```rust
struct DirectPresentFrame {
    source_pin: u64,
    fallback_target_pin: u64,
    source_id: DrawableId,
    candidate: PresentScanoutCandidate,
    fallback_target: PaintTarget,
    event: yserver_core::backend::CompletedPresentEvent,
    awaiting_outputs: HashSet<usize>,
    fb: Option<std::sync::Arc<crate::drm::modeset::DirectScanoutProbeFramebuffer>>,
    is_async: bool,
}
```

2. Update `DirectPresentFrame::for_tests()`:
```rust
    is_async: false,
```

3. Update `prepare_direct_frame`:
```rust
    let is_async = (candidate.options
        & (yserver_core::present_scheduler::PRESENT_OPTION_ASYNC
            | yserver_core::present_scheduler::PRESENT_OPTION_ASYNC_MAY_TEAR))
        != 0;
    DirectPresentFrame {
        source_pin,
        fallback_target_pin,
        source_id,
        candidate,
        fallback_target,
        event,
        awaiting_outputs: (0..self.platform.outputs.len()).collect(),
        fb: Some(std::sync::Arc::clone(fb)),
        is_async,
    }
```

4. Update `try_present_direct` and `submit_chain_direct_frame` calls to `submit_direct_scanout`:
```rust
    // In try_present_direct:
    if let Err(error) = crate::drm::modeset::submit_direct_scanout(
        &self.platform.device,
        fb.handle(),
        &plane_states,
        frame.is_async,
    ) { ... }

    // In submit_chain_direct_frame:
    if let Err(error) = crate::drm::modeset::submit_direct_scanout(
        &self.platform.device,
        fb.handle(),
        &plane_states,
        frame.is_async,
    ) { ... }
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --package yserver --lib kms::render::backend::tests::direct_present_frame_captures_async_option_flag`
Expected: PASS

- [ ] **Step 5: Run existing direct scanout tests to ensure zero regression on synced path**

Run: `cargo test --package yserver --lib kms::render::backend::tests`
Expected: All PASS

- [ ] **Step 6: Commit**

```bash
git add crates/yserver/src/kms/render/backend.rs
git commit -m "feat(scanout): track is_async on DirectPresentFrame and pass to submit_direct_scanout"
```

---

### Task 4: Test Suite & Comprehensive Regression Verification

**Files:**
- Test: `crates/yserver/src/kms/render/backend.rs`
- Test: `crates/yserver-core/src/present_scheduler.rs`

- [ ] **Step 1: Add state transition unit tests for async queued promotion**

In `crates/yserver/src/kms/render/backend.rs` tests module:
Verify that an arriving async present while `pending` is some replaces `queued`, skips the old frame, and chain-submits cleanly.

- [ ] **Step 2: Run all workspace tests**

Run: `cargo test --all-targets`
Expected: All tests PASS

- [ ] **Step 3: Verify formatting and clippy clean**

Run: `cargo +nightly fmt --check`
Run: `cargo clippy --all-targets -- -D warnings`
Expected: Zero warnings, zero formatting errors

- [ ] **Step 4: Commit**

```bash
git add crates/yserver/src/kms/render/backend.rs crates/yserver/src/drm/modeset.rs
git commit -m "test(scanout): pin async direct scanout state transitions and clippy clean"
```

---

### Task 5: Hardware A/B Validation & Telemetry Capture

**Files:**
- Tool/Script: `tools/yserver-cinnamon-hw-cs2.sh` (or live gameplay session)
- Documentation: `docs/superpowers/findings/2026-08-11-cs2-fullscreen-novsync-pageflip-collapse.md`

- [ ] **Step 1: Execute hardware validation run with CS2 / Vulkan game (VSync OFF)**
- [ ] **Step 2: Verify in-game FPS counter reaches unconstrained rate (300-400+ FPS)**
- [ ] **Step 3: Verify screen tearing is present and composed unflips remain 0**
- [ ] **Step 4: Document results in findings doc**
- [ ] **Step 5: Commit documentation**

```bash
git add docs/superpowers/findings/2026-08-11-cs2-fullscreen-novsync-pageflip-collapse.md
git commit -m "docs(scanout): record direct scanout async tearing hardware validation"
```
