# XRender Submit Coalescing & Desktop Compositor Performance Design Specification (Issue #115)

- **Date:** 2026-08-23
- **Status:** Approved for Implementation
- **Branch:** `perf/115-cinnamon-xrender-submit-coalescing`
- **Related Issues & Findings:**
  - Issue #115: *Poor performance on yserver and graphical glitches*
  - `docs/superpowers/findings/2026-08-11-yserver-leak-cinnamon-regression-and-transparency-bug.md`
  - `docs/superpowers/specs/2026-05-15-rendering-model-v2.md`
  - `docs/superpowers/specs/2026-05-23-frame-builder-submit-rate-design.md`

---

## 1. Problem Statement & Root Cause Analysis

Quantitative analysis of hardware telemetry from Cinnamon/Muffin sessions (`issue_115/yserver-hw-cinnamon.log` and `issue_115/yserver-cinnamon.submit.tsv`) revealed that desktop animation stutter is caused by a massive **Vulkan submit storm** and **core loop CPU saturation**:

1. **Submit Storm:** Over **185,190 submits** in a 120-second session (~1,534 submits/s average), peaking at **12,285 `vkQueueSubmit2` calls in a single second** during window animations.
   - `put_image`: 43,855 (23.7%)
   - `render_composite`: 40,626 (21.9%)
   - `render_fill`: 40,425 (21.8%)
   - `render_traps`: 33,661 (18.2%)
   - `composite_glyphs`: 21,417 (11.6%)
2. **`RedirectSourceBoundary` Frame Splitting:** In `crates/yserver/src/kms/render/engine.rs`, `render_composite` forces a full frame close (`close_open_frame(RedirectSourceBoundary)`) before and after every composite where the source is an active redirect backing (`store.is_active_redirect_target(src_id)`). Because Cinnamon (Muffin) redirects all windows to backings, virtually every window and menu composite forces an immediate queue submit, completely bypassing the `FrameBuilder` aggregation pipeline.
3. **Core Loop Saturation & Framerate Drop:** The single-threaded core loop spends **58% to 68.7% of CPU time** (up to 686 ms/s) handling XRender requests (`op133` consuming ~500 ms/s alone across ~35,000 req/s). This starves the event loop, causing the presentation/page-flip rate to collapse from **120 FPS down to 35–50 FPS** during desktop animations.
4. **Massive Pixmap Churn:** Muffin creates and frees ~60,000 pixmaps in 2 minutes (~500/s), creating contention and recycling overhead.

---

## 2. Goals & Non-Goals

### Goals
- **Eliminate Submit Storm:** Replace queue submission boundaries (`RedirectSourceBoundary`) with intra-command-buffer `vkCmdPipelineBarrier2` memory synchronization (RAW hazard prevention).
- **Multi-Primitive Frame Aggregation:** Allow interleaved `RenderComposite`, `RenderFill`, `RenderTraps`, `CompositeGlyphs`, `PutImage`, and `CopyArea` operations within the open `FrameBuilder` frame without intermediate queue flushes.
- **Sub-100 Submits/sec on Compositors:** Reduce submit rate under active Cinnamon animations from >12,000 submits/s to under 120 submits/s (1 submit per pacing tick / VBlank).
- **Maintain Smooth 120/144/165 Hz Pacing:** Free up core loop CPU headroom (reducing request processing time from ~650 ms/s to <100 ms/s), sustaining full display refresh rate during window dragging, menu popups, and shell animations.
- **Zero Graphical Glitches:** Ensure exact Read-After-Write (RAW) coherency for redirected window updates, submenus, and popups without rendering artifacts or stale textures.
- **Fast-Path Pixmap Pool:** Ensure O(1) allocation and recycling for frequent temporary pixmaps under compositor workloads.

### Non-Goals
- Changing the single-threaded architecture of `yserver-core`.
- Altering the direct scanout tearing path for unredirected fullscreen games (preserved as implemented in `feat/direct-scanout-async-tearing`).

---

## 3. Architecture & Technical Design

### 3.1. Intra-Frame Barrier Synchronization for Redirect Sources

Instead of flushing the queue before/after sampling a redirected backing, `FrameBuilder` will track dirty write states on drawables within the open frame:

```
[Window / Client Paint: PutImage / Fill / Traps]
                       |
                       v
         Storage layout marked as DIRTY_WRITE
       (COLOR_ATTACHMENT / TRANSFER_DST_OPTIMAL)
                       |
[Compositor / Muffin: RenderComposite (src = Window)]
                       |
                       v
       Is src drawable DIRTY_WRITE in current CB?
              /                  \
            Yes                   No
            /                       \
           v                         v
 Insert vkCmdPipelineBarrier2     No barrier needed
 (COLOR_ATTACHMENT -> SHADER_READ)  (Already in SHADER_READ)
           \                         /
            v                       v
         Execute Composite in SAME Command Buffer
```

#### Pipeline Barrier Specification:
- `srcStageMask = VK_PIPELINE_STAGE_2_COLOR_ATTACHMENT_OUTPUT_BIT | VK_PIPELINE_STAGE_2_TRANSFER_BIT`
- `srcAccessMask = VK_ACCESS_2_COLOR_ATTACHMENT_WRITE_BIT | VK_ACCESS_2_TRANSFER_WRITE_BIT`
- `dstStageMask = VK_PIPELINE_STAGE_2_FRAGMENT_SHADER_BIT`
- `dstAccessMask = VK_ACCESS_2_SHADER_READ_BIT`
- `oldLayout = VK_IMAGE_LAYOUT_COLOR_ATTACHMENT_OPTIMAL` (or `TRANSFER_DST_OPTIMAL`)
- `newLayout = VK_IMAGE_LAYOUT_SHADER_READ_ONLY_OPTIMAL`

This provides 100% GPU-level hardware synchronization inside the same command buffer, completely eliminating `RedirectSourceBoundary` queue flushes while guaranteeing that popups, menus, and window contents never sample stale data.

---

### 3.2. Unified 2D Primitive Coalescing in `engine.rs`

1. **Staging Upload Batching (`put_image` / `shm_put_image`):**
   - Pool staging memory buffers and record `vkCmdCopyBufferToImage` into the open frame command buffer without triggering a batch flush or frame close.
   - Transition target images using barrier tracking.
2. **Renderpass End / Transition Coalescence:**
   - When switching from `RenderFill` to `RenderComposite` or `RenderTraps`, end the dynamic rendering pass (`vkCmdEndRendering`) without closing the `FrameBuilder` or submitting to the Vulkan queue.
   - Subsequent ops append to the existing command buffer until the pacing timer, explicit sync fence, or frame completion triggers the submission.

---

### 3.3. High-Churn Pixmap Pool O(1) Fast-Path

1. **Exact-Fit Bucket L1 Cache:** Keep a fast L1 cache of available pre-allocated Vulkan images indexed by `(width, height, depth, format)` in `crates/yserver/src/kms/render/store.rs`.
2. **Immediate Return on Free:** When `FreePixmap` is called for a standalone client pixmap with refcount 1 and no active fences, return the storage immediately to the L1 bucket without deallocating Vulkan memory or invoking DRM ioctls.

---

## 4. Verification & Success Metrics

1. **Submit Rate:** `submit_trace.tsv` submits per second during Cinnamon animations drops from **>12,000/s to <120/s**.
2. **Core Loop CPU Time:** `req_time` during heavy animations drops from **~650 ms/s (68%) to <150 ms/s (<15%)**.
3. **Pageflip Rate:** `page_flip/s` holds solid at **120 / 144 / 165 FPS** during window dragging and shell animations (no drop to 35 FPS).
4. **Visual Quality:** 0 visual glitches, 0 transparent windows, 0 missing popup submenus in XFCE/MATE/Cinnamon.
5. **CI Compliance:** `cargo clippy --all-targets -- -D warnings` and `cargo test` clean.
