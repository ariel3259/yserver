# XRender Submit Coalescing & Desktop Compositor Performance Design Specification (Issue #115)

- **Date:** 2026-08-23 (Updated after Adversarial Review)
- **Status:** Approved for Implementation
- **Branch:** `perf/115-cinnamon-xrender-submit-coalescing`
- **Related Issues & Findings:**
  - Issue #115: *Poor performance on yserver and graphical glitches*
  - `docs/superpowers/findings/2026-08-11-yserver-leak-cinnamon-regression-and-transparency-bug.md`
  - `docs/superpowers/specs/2026-05-15-rendering-model-v2.md`
  - `docs/superpowers/specs/2026-05-23-frame-builder-submit-rate-design.md`
  - `docs/superpowers/specs/2026-05-24-frame-builder-phase-b-design.md`

---

## 1. Problem Statement & Root Cause Analysis

Quantitative analysis of hardware telemetry from Cinnamon/Muffin sessions (`issue_115/yserver-hw-cinnamon.log` and `issue_115/yserver-cinnamon.submit.tsv`) revealed that desktop animation stutter is caused by a massive **Vulkan submit storm** and **core loop CPU saturation**:

1. **Submit Storm:** Over **185,190 submits** in a 120-second session (~1,534 submits/s average), peaking at **12,285 `vkQueueSubmit2` calls in a single second** during window animations.
   - `put_image`: 43,855 (23.7%)
   - `render_composite`: 40,626 (21.9%)
   - `render_fill`: 40,425 (21.8%)
   - `render_traps`: 33,661 (18.2%)
   - `composite_glyphs`: 21,417 (11.6%)
2. **Obsolete `RedirectSourceBoundary` Workaround:** In `crates/yserver/src/kms/render/engine.rs` (lines 6848–6858, 6877–6883), `render_composite` forces a full frame close (`close_open_frame(RedirectSourceBoundary)`) before and after every composite where the source is an active redirect backing (`store.is_active_redirect_target(src_id)`).
   - **Historical Context:** This hack was introduced on 2026-07-13 to fix an XFCE/xfwm popup submenu glitch. At that time, `last_render_ticket` was only committed on frame close. If a short-lived popup was created, composited, and freed within the same frame, `FreePixmap` saw no active ticket and immediately destroyed the Vulkan backing before the CB was submitted (Use-After-Free).
   - **Current Reality:** In Phase B.3 (Task 12), `FrameBuilder` implemented **eager ticket stamping at op-append time**. `FreePixmap` now immediately detects the active ticket and safely routes destruction to `pending_retire`.
   - **Layout Synchronization:** Furthermore, `DstPassSession` (Slice-2 Dynamic Rendering) already automatically manages layout transitions (`COLOR_ATTACHMENT` -> `SHADER_READ_ONLY_OPTIMAL`) whenever the composite destination changes or a pass ends.
   - **The Defect:** Because Cinnamon (Muffin) redirects all windows to backings, this obsolete hack turns every window, shadow, and icon composite into two immediate queue submissions, defeating `FrameBuilder` aggregation completely and causing a 12,000 submits/s storm.
3. **Core Loop Saturation & Framerate Collapse:** The single-threaded core loop spends **58% to 68.7% of CPU time** (up to 686 ms/s) dispatching fragmented XRender submissions. This starves the event loop, causing the presentation/page-flip rate to collapse from **120 FPS down to 35–50 FPS** during desktop animations.

---

## 2. Goals & Non-Goals

### Goals
- **Eliminate Submit Storm:** Remove the obsolete `RedirectSourceBoundary` frame-closing hack from `render_composite` and rely on `FrameBuilder`'s native eager ticket stamping and `DstPassSession` layout transitions.
- **Natural FrameBuilder Coalescing:** Allow all consecutive `RenderComposite`, `RenderFill`, `RenderTraps`, `CompositeGlyphs`, `PutImage`, and `CopyArea` operations within a pacing interval to coalesce into the open `FrameBuilder` command buffer.
- **Sub-120 Submits/sec on Compositors:** Reduce submit rate under active Cinnamon animations from >12,000 submits/s to <120 submits/s (1 submit per pacing tick / VBlank).
- **Maintain Smooth 120/144/165 Hz Pacing:** Free up core loop CPU headroom (reducing request processing time from ~650 ms/s to <100 ms/s), sustaining full display refresh rate during window dragging, menu popups, and shell animations.
- **Zero Graphical Glitches:** Ensure 100% glitch-free rendering for popups, menus, and translucent surfaces across XFCE, MATE, and Cinnamon.

### Non-Goals
- Changing the single-threaded architecture of `yserver-core`.
- Altering the direct scanout tearing path for unredirected fullscreen games (preserved as implemented in `feat/direct-scanout-async-tearing`).
- Bypassing the Vulkan fence lifecycle in `PixmapPool` (fences must strictly protect against WAW/RAW aliasing).

---

## 3. Architecture & Technical Design

### 3.1. Removal of `RedirectSourceBoundary` & Unification with `DstPassSession`

1. **Delete Obsolete Boundary Code:**
   In `crates/yserver/src/kms/render/engine.rs`, remove:
   ```rust
   // Remove lines 6848-6858 and 6877-6883:
   let src_is_redirect_backing = match &src { ... };
   if src_is_redirect_backing {
       self.close_open_frame(store, platform, CloseReason::RedirectSourceBoundary)?;
   }
   ```
2. **Clean Up `CloseReason` Enum:**
   In `crates/yserver/src/kms/render/frame_builder.rs`, deprecate / remove `CloseReason::RedirectSourceBoundary` (and update telemetry tracking).
3. **Coherency Guarantees:**
   - **Ticket Lifecycle (UAF Protection):** Protected by Phase B.3 Task 12 eager stamping. When `render_composite_via_frame_builder` records an op, `store.touch_render_fence` stamps the frame ticket immediately. If `FreePixmap` occurs before the frame closes, `handle_free_pixmap` defers deallocation until `poll_retired` confirms GPU completion.
   - **Layout Transitions (RAW Protection):** Managed by `DstPassSession`. When a window backing was previously a render target (`COLOR_ATTACHMENT_OPTIMAL`) and is subsequently sampled as a composite source, `DstPassSession::step` closes the previous pass and inserts the pipeline barrier to `SHADER_READ_ONLY_OPTIMAL` before `vkCmdBeginRendering` starts the new pass.

---

### 3.2. 2D Primitive Coalescing & Staging Buffer Pacing

1. **Staging Pool Reuse:**
   `put_image` utilizes `inner.staging_pool.acquire(...)`. Pinned staging buffers are added to `open.pinned_staging`.
2. **Pin Ceiling Safeguard:**
   The existing `would_exceed_pin_ceiling()` limit (`max_pinned_resources_per_frame = 1024`) provides a safe upper bound. Under high-throughput bursts (>1,000 ops/tick), `FrameBuilder` gracefully closes and submits at the 1,024-pin boundary (~1–2 submits/s), preserving correctness without thrashing.

---

## 4. Verification & Success Metrics

1. **Submit Rate:** `submit_trace.tsv` submits per second during Cinnamon animations drops from **>12,000/s to <120/s**.
2. **Core Loop CPU Time:** `req_time` during heavy animations drops from **~650 ms/s (68%) to <150 ms/s (<15%)**.
3. **Pageflip Rate:** `page_flip/s` holds solid at **120 / 144 / 165 FPS** during window dragging and shell animations (no drop to 35 FPS).
4. **Visual Quality:** 0 visual glitches, 0 transparent windows, 0 missing popup submenus in XFCE/MATE/Cinnamon.
5. **CI Compliance:** `cargo clippy --all-targets -- -D warnings` and `cargo test` clean.
