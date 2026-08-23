# XRender Submit Coalescing & Desktop Compositor Performance Implementation Plan (Issue #115)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Eliminate the Vulkan submit storm (>12,000 submits/s) and core loop CPU saturation (68% CPU time spent on XRender) on composited desktop environments (Cinnamon/Muffin, MATE, XFCE) to maintain a smooth 120/144/165 FPS refresh rate without animation stutter or graphical glitches.

**Architecture:**
1. **Intra-CB Pipeline Barrier Synchronization:** Replace `RedirectSourceBoundary` queue flushes (`close_open_frame`) with intra-command-buffer `vkCmdPipelineBarrier2` memory transitions from `COLOR_ATTACHMENT_OPTIMAL` / `TRANSFER_DST_OPTIMAL` to `SHADER_READ_ONLY_OPTIMAL` when compositing from redirected window backings.
2. **Unified 2D Primitive Coalescing:** Aggregate consecutive `RenderFill`, `RenderComposite`, `RenderTraps`, `CompositeGlyphs`, and `PutImage` operations inside the open `FrameBuilder` frame without intermediate queue submissions.
3. **High-Churn Pixmap Recycling Fast-Path:** Implement O(1) L1 bucket recycling for temporary pixmaps to eliminate DRM/driver allocation overhead under high churn (~500 creates/frees per second).

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

- `crates/yserver/src/kms/render/engine.rs` — Eliminate `RedirectSourceBoundary` submit boundaries; implement intra-frame barrier insertion on write-to-sample transitions; coalesce dynamic rendering passes without closing the open frame.
- `crates/yserver/src/kms/render/frame_builder.rs` — Extend open frame tracking for intra-frame dirty write drawables and barrier records.
- `crates/yserver/src/kms/render/store.rs` — O(1) L1 fast cache for temporary pixmap storage reuse under high churn.
- `crates/yserver/src/kms/render/backend.rs` — Plumbing for aggregated 2D render dispatches and telemetry validation.

---

### Task 1: Intra-Frame Barrier Tracking in `FrameBuilder`

**Files:**
- Modify: `crates/yserver/src/kms/render/frame_builder.rs`
- Test: `crates/yserver/src/kms/render/frame_builder.rs` (unit tests)

**Objectives:**
- Track drawables written within the current open frame (`written_in_frame: HashSet<DrawableId>`).
- Provide `ensure_sampled_readable(target_id)` helper that records a `vkCmdPipelineBarrier2` transition to `SHADER_READ_ONLY_OPTIMAL` if the target was written in the same frame, clearing the write-dirty state.

- [ ] **Step 1: Write failing unit tests for `written_in_frame` and `ensure_sampled_readable`**
- [ ] **Step 2: Run tests to verify failure**
- [ ] **Step 3: Implement barrier tracking in `FrameBuilder`**
- [ ] **Step 4: Run tests to verify they pass**
- [ ] **Step 5: Run clippy and format**
- [ ] **Step 6: Commit**

---

### Task 2: Replace `RedirectSourceBoundary` Submit Flushes with Intra-CB Barriers in `engine.rs`

**Files:**
- Modify: `crates/yserver/src/kms/render/engine.rs`
- Test: `crates/yserver/src/kms/render/engine.rs` (unit tests)

**Objectives:**
- Remove the before/after `close_open_frame(RedirectSourceBoundary)` in `render_composite`.
- Integrate `ensure_sampled_readable` for `src` and `mask` drawables when binding descriptor sets in `render_composite_via_frame_builder`.
- Preserve XFCE/Muffin popup and submenu visual correctness without forcing queue submissions.

- [ ] **Step 1: Write unit tests verifying that consecutive composites from redirected backings share the same open frame**
- [ ] **Step 2: Update `render_composite` in `engine.rs` to remove `close_open_frame(RedirectSourceBoundary)` and apply intra-frame barriers**
- [ ] **Step 3: Run unit and integration tests to verify pass**
- [ ] **Step 4: Run clippy and format**
- [ ] **Step 5: Commit**

---

### Task 3: 2D Primitives Dynamic Rendering Coalescing

**Files:**
- Modify: `crates/yserver/src/kms/render/engine.rs`
- Test: `crates/yserver/src/kms/render/engine.rs`

**Objectives:**
- Allow `put_image`, `render_fill`, `render_traps`, and `composite_glyphs` to record into the open frame without closing the frame or forcing premature `flush_render_batch`.
- Properly end dynamic rendering passes (`vkCmdEndRendering`) when switching draw targets/pipelines without submitting the command buffer.

- [ ] **Step 1: Write tests for multi-primitive batching across `put_image`, `render_fill`, `render_composite`**
- [ ] **Step 2: Update batching logic in `engine.rs`**
- [ ] **Step 3: Verify all render tests pass**
- [ ] **Step 4: Run clippy and format**
- [ ] **Step 5: Commit**

---

### Task 4: Pixmap Pool L1 Fast-Path for High-Churn Workloads

**Files:**
- Modify: `crates/yserver/src/kms/render/store.rs`
- Test: `crates/yserver/src/kms/render/store.rs`

**Objectives:**
- Add O(1) exact-match L1 cache for recycled pixmap storages.
- Fast-path `create_pixmap` and `free_pixmap` when dimensions and formats match existing pooled buffers.

- [ ] **Step 1: Write benchmark / unit tests for pixmap churn**
- [ ] **Step 2: Implement L1 fast-path in `store.rs`**
- [ ] **Step 3: Run tests and verify performance**
- [ ] **Step 4: Run clippy and format**
- [ ] **Step 5: Commit**

---

### Task 5: Hardware Validation & Regression Testing

**Files:**
- Test suite: `cargo test --all-targets`
- Clippy: `cargo clippy --all-targets -- -D warnings`
- Telemetry trace validation script

- [ ] **Step 1: Run full test suite across workspace**
- [ ] **Step 2: Run strict clippy and rustfmt**
- [ ] **Step 3: Verify submit trace reduction in simulated multi-window composite trace**
