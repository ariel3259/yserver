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

- [x] **Step 1: Write the failing unit test for `direct_scanout_commit_flags`**
- [x] **Step 2: Run test to verify failure**
- [x] **Step 3: Implement `direct_scanout_commit_flags` and update `submit_direct_scanout`**
- [x] **Step 4: Run test to verify it passes**
- [x] **Step 5: Commit**

---

### Task 2: Advertise `async_may_tear` and `flip_path` in `backend.present_capabilities`

**Files:**
- Modify: `crates/yserver/src/kms/render/backend.rs`
- Test: `crates/yserver/src/kms/render/backend.rs`

- [x] **Step 1: Write unit test for `present_capabilities` async_may_tear**
- [x] **Step 2: Update `present_capabilities` implementation in `backend.rs`**
- [x] **Step 3: Run test to verify it passes**
- [x] **Step 4: Commit**

---

### Task 3: Propagate `is_async` through `DirectPresentFrame` & `backend.rs` State Machine

**Files:**
- Modify: `crates/yserver/src/kms/render/backend.rs` (`DirectPresentFrame`, `prepare_direct_frame`, `submit_chain_direct_frame`, `try_present_direct`)

- [x] **Step 1: Write unit test for `DirectPresentFrame` async flag detection**
- [x] **Step 2: Add `is_async` field to `DirectPresentFrame` and update call sites**
- [x] **Step 3: Run tests to verify pass and zero regression on synced path**
- [x] **Step 4: Commit**

---

### Task 4: Test Suite & Comprehensive Regression Verification

**Files:**
- Test: `crates/yserver/src/kms/render/backend.rs`
- Test: `crates/yserver-core/src/present_scheduler.rs`

- [x] **Step 1: Add state transition unit tests for async queued promotion**
- [x] **Step 2: Run all workspace tests**
- [x] **Step 3: Verify formatting and clippy clean**
- [x] **Step 4: Commit**

---

### Task 5: Direct Scanout Pipeline Unconstraining & Latency Reduction

- [x] **Step 1: Dispatch async presents as `ExecuteNow` in scheduler**
- [x] **Step 2: Signal DRI3 wake immediately on `complete_queued_as_skip` to avoid swapchain stalls**
- [x] **Step 3: Pick highest available refresh rate and support `YSERVER_MODE=WxH@Hz`**
- [x] **Step 4: Flush present completions immediately before epoll poll in event loop**
- [x] **Step 5: Commit**

---

### Task 6: In-Flight Fence Bypass & Hardware Cursor Stutter Elimination

- [x] **Step 1: Bypass user-space sync-file export and epoll deferral during active direct scanout**
- [x] **Step 2: Implement `XFixesHideCursor` and `XFixesShowCursor` to cleanly unbind cursor plane**
- [x] **Step 3: Deduplicate cursor plane movement coordinates to eliminate redundant ioctls**
- [x] **Step 4: Move hardware cursor updates off-thread to prevent mouse movement stutter on NVIDIA DRM**
- [x] **Step 5: Commit**

---

### Task 7: Fullscreen Redirected Windows Admission & Unflip Stabilization

- [x] **Step 1: Admit fullscreen redirected windows with root coverage into direct scanout**
- [x] **Step 2: Remove premature direct unflip requests to eliminate desktop background flicker**
- [x] **Step 3: Run all workspace tests and verify zero regressions**
- [x] **Step 4: Commit**
