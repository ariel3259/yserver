# XRender Submit Coalescing & Desktop Compositor Performance Implementation Plan (Issue #115)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Eliminate the Vulkan submit storm (>12,000 submits/s) and core loop CPU saturation (68% CPU time spent on XRender) on composited desktop environments (Cinnamon/Muffin, MATE, XFCE) by removing the obsolete `RedirectSourceBoundary` submit hack and allowing full `FrameBuilder` coalescing, sustaining a smooth 120/144/165 FPS refresh rate without animation stutter or graphical glitches.

**Architecture:**
1. **Remove `RedirectSourceBoundary` Workaround:** Eliminate the obsolete before/after `close_open_frame` hack in `render_composite`, allowing consecutive composites of redirected window backings to naturally batch within the open `FrameBuilder` frame.
2. **Verify Eager Ticket Stamping & Layout Synchronization:** Validate that `FrameBuilder` eager ticket stamping (Phase B.3 Task 12) and `DstPassSession` dynamic rendering layout transitions (`COLOR_ATTACHMENT` -> `SHADER_READ_ONLY_OPTIMAL`) guarantee 100% glitch-free rendering and UAF protection across rapid create/composite/free cycles.
3. **Telemetry & CloseReason Cleanup:** Remove `CloseReason::RedirectSourceBoundary` from telemetry and logging.

**Tech Stack:** Rust, Vulkan 1.3 / Ash, `yserver-core`, `yserver`.

---

## Global Constraints

- Spec: `docs/superpowers/specs/2026-08-23-xrender-submit-coalescing-cinnamon-perf-design.md`.
- Conforms with: `docs/high-level-design.md`, `AGENTS.md` (no libc linuxisms, regular clippy clean: `cargo clippy --all-targets -- -D warnings`, formatting: `cargo +nightly fmt`).
- Spec compliance: X11 / XRender / MIT-SHM protocol compliance strictly preserved.
- Zero graphical regressions: Popups, submenus, shadows, transparent windows, and redirected surfaces must remain 100% glitch-free.
- TDD: write unit tests first, verify failure, implement, verify pass, commit.

---

## File Structure

- `crates/yserver/src/kms/render/engine.rs` — Remove `RedirectSourceBoundary` submit boundaries in `render_composite`.
- `crates/yserver/src/kms/render/frame_builder.rs` — Remove `CloseReason::RedirectSourceBoundary`.
- `crates/yserver/src/kms/render/telemetry.rs` — Clean up telemetry bucket formatting for removed close reason.

---

### Task 1: Write Regression & Batching Unit Tests for Redirected Composites

**Files:**
- Modify: `crates/yserver/src/kms/render/engine.rs` (unit test module)

**Objectives:**
- Write a unit test `redirected_source_composites_coalesce_in_single_open_frame`: verify that multiple consecutive `render_composite` calls with redirected window sources execute within a single open frame without closing or submitting.
- Write a unit test `redirected_source_composite_eager_ticket_prevents_uaf_on_free_pixmap`: verify that creating a redirected window, compositing from it, and calling `free_pixmap` within the same open frame retains the backing in `pending_retire` until frame completion.

- [ ] **Step 1: Write unit tests in `engine.rs`**
- [ ] **Step 2: Run tests to verify failure with `RedirectSourceBoundary` present**

---

### Task 2: Remove `RedirectSourceBoundary` in `engine.rs`

**Files:**
- Modify: `crates/yserver/src/kms/render/engine.rs:6848-6885`

**Objectives:**
- Remove the before/after `self.close_open_frame(store, platform, CloseReason::RedirectSourceBoundary)` checks.
- Let `render_composite` dispatch directly to `render_composite_via_frame_builder`.

- [ ] **Step 1: Remove the `RedirectSourceBoundary` checks in `render_composite`**
- [ ] **Step 2: Run unit tests to verify they pass**
- [ ] **Step 3: Run full engine test suite (`cargo test -p yserver --lib`)**

---

### Task 3: Clean Up `CloseReason` and Telemetry

**Files:**
- Modify: `crates/yserver/src/kms/render/frame_builder.rs`
- Modify: `crates/yserver/src/kms/render/telemetry.rs`

**Objectives:**
- Remove or deprecate `CloseReason::RedirectSourceBoundary`.
- Update telemetry bucket counters and formatting.

- [ ] **Step 1: Update `frame_builder.rs` and `telemetry.rs`**
- [ ] **Step 2: Run `cargo clippy --all-targets -- -D warnings`**
- [ ] **Step 3: Run `cargo +nightly fmt`**
- [ ] **Step 4: Commit**

---

### Task 4: Full Workspace Verification & Benchmarking

**Files:**
- Run: `cargo test --all-targets`
- Run: `cargo clippy --all-targets -- -D warnings`

- [ ] **Step 1: Run complete workspace test suite**
- [ ] **Step 2: Run strict clippy and rustfmt**
