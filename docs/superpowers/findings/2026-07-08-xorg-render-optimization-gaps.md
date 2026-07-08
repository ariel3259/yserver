# Xorg rendering-optimization gaps vs yserver v2

**Date:** 2026-07-08
**Method:** Four parallel code-reading agents compared `/home/jos/Projects/xserver`
(glamor, RENDER, damage/Present/Composite, EXA/pixmap memory) against the yserver
v2 Vulkan renderer. Every claim below was verified against actual source on **both**
sides — file:line references are load-bearing, not illustrative. Items where yserver
is already at parity or ahead are listed in the final section so they are not
re-investigated.

## TL;DR

The highest-ROI next rendering work is **not** submit aggregation (see the "Task 3
was a phantom" note below). It is:

1. **Glyph draw batching** — yserver issues one `vkCmdDraw` per glyph; its own
   trapezoid path already proves the engine can do a single instanced draw per run.
   ✅ **DONE** (`ae5f6bc7`, 2026-07-08).
2. **A1→A8 glyph-expansion caching** — cheap, localized; removes per-frame CPU
   waste on all repeated text. ✅ **DONE** (`c35ac33f`, 2026-07-08).

Both hit the heaviest real desktop workload (GTK/Pango text) and both are
low-risk. Everything else is either bigger, structurally blocked, or already
tracked in `docs/superpowers/plans/2026-05-20-stage-5-make-v2-fast.md`.

---

## Tier 1 — high value, low risk, in-repo template exists

### 1. Batch glyph draws into one instanced draw per run  ✅ DONE (`ae5f6bc7`, 2026-07-08)
> Implemented as scoped: `GlyphInstanceData` (INSTANCE-rate, atlas texel coords),
> per-run `TextPushConsts` (viewport + atlas_extent + foreground), rewritten
> `text.vert.glsl`, instance buffer built + pinned at record time (like the trap
> `vertex_pool_pin`), one `vkCmdDraw(4, N)` per scissor. Codex-reviewed; lavapipe +
> HW-smoke (st/xterm/wezterm on wmaker) confirmed. Scope:
> `docs/superpowers/plans/2026-07-08-glyph-batching-and-a1-cache-scope.md`.

*Flagged independently by both the glamor and RENDER explorers.*

- **yserver:** `record_text_run_scissored` (`crates/yserver/src/kms/vk/ops/text.rs:189-221`)
  loops `for scissor_rect in scissors { for g in glyphs { cmd_push_constants(..); cmd_draw(cb, 4, 1, 0, 0) } }`
  — **one `vkCmdPushConstants` + one `vkCmdDraw` per glyph, per clip-rect** (nested).
  Both `image_text` and `composite_glyphs` funnel through this recorder.
- **Xorg/glamor:** `glamor_composite_glyphs` queues every glyph of a run into one
  VBO and flushes with a single `glDrawArraysInstanced(GL_TRIANGLE_STRIP, 0, 4, nglyph)`
  per scissor rect (`glamor/glamor_composite_glyphs.c:262-361,372-539`), per-glyph
  position/atlas-offset fetched via a per-instance vertex attribute.
- **Why it's the top pick:** yserver **already does exactly this for trapezoids** —
  `emit_recorded_render_traps_or_tris_into_cb` renders N traps with one
  `cmd_draw(cb, 4, rt.instance_count, 0, 0)` into a reusable `mask_scratch` image
  (`crates/yserver/src/kms/v2/engine.rs:10405`). The instanced-draw-into-mask
  plumbing exists; the glyph path just doesn't use it. GTK/Pango text is the single
  heaviest RENDER traffic source on a MATE desktop.
- **Impact:** For dense text (terminals, IDEs, menus) this is N per-glyph
  command-buffer entries + N driver-side draw-state validations where one instanced
  draw would do. FrameBuilder already batches at the *submission* level, so this is
  not a syscall storm — it's CPU command-recording + GPU vertex-dispatch overhead.

### 2. Gate A1→A8 glyph expansion on cache-miss  ✅ DONE (`c35ac33f`, 2026-07-08)
> Fixed as described. New `kms::v2::glyph_pixels` module carries
> `GlyphPixels{A8,A1Wire}` + `to_a8` (borrow A8, lazily expand A1) + the relocated
> `expand_a1_glyph_to_a8` and its #77 tests; `CompositeGlyphInput.pixels` is now the
> enum; the backend forwards raw glyphset bytes and the engine expands A1 only in the
> atlas-miss branch. A8 path is an unchanged zero-copy borrow. HW-confirmed on wmaker
> (st+Terminus A1 upright — #77 stays fixed; xterm/wezterm/gkrellm A8 no-regression).
> Scope: `docs/superpowers/plans/2026-07-08-glyph-batching-and-a1-cache-scope.md`.

- **yserver:** `render_composite_glyphs`'s wire-parse loop calls
  `expand_a1_glyph_to_a8(&glyph.pixels, gw, gh)` **unconditionally, on every
  `CompositeGlyphs` request** (`crates/yserver/src/kms/v2/backend.rs:15959-15969`;
  fn at `backend.rs:18603`). This happens in the protocol parse, **before** the
  engine's atlas-cache lookup (`GlyphKey`/`lookup(key)` at `engine.rs:6339`) runs.
  So a glyph already resident in the GPU atlas still pays a full CPU A1→A8 bit-unpack
  every time it appears. ARGB32-source glyphs are already correctly pre-converted to
  A8 once at ingest (`backend.rs:15970-15974`); A1 is the straggler.
- **Xorg:** EXA/glamor gate conversion+upload on a cache-miss check — a glyph is
  converted to GPU-native format exactly once, ever (`exa/exa_glyphs.c:425-504`,
  `glamor/glamor_composite_glyphs.c:449-466`).
- **Fix shape:** move the `expand_a1_glyph_to_a8` call behind the atlas lookup / only
  run it on new uploads. Cheap and well-localized.

---

## Tier 2 — medium value, medium effort

### 3. Composite→CopyArea "is this actually a copy" fast path  ✅ DONE (clip-aware, 2026-07-08)
> Landed as the **clip-aware** version matching glamor: `composite_is_copy_equivalent`
> (`PictOpSrc` + drawable source + no mask/transform/repeat + exact matching format)
> gates the reroute, and `clip_copy_rect` intersects each composite rect with the picture
> clip so each rect∩clip box is copied via native `vkCmdCopyImage` (`copy_area`), matching
> `glamor_composite_clipped_region` (which hands all clip boxes to `glamor_copy`). An
> unclipped-only first cut fired **0×** on real desktops (fvwm + mate) — the near-miss
> telemetry showed the eligible population is almost entirely *clipped*, so clip-awareness
> is the whole point. HW-validated on mate (bee: `composite_copy_fastpath/s` median 47 /
> peak 184; visually smear-free under increased `copy_area` load, on top of the negative-
> offset clamp fix `0d9972ad`). Multi-region batching (gap #5: one `vkCmdCopyImage` over N
> boxes + one barrier pair) still open — currently one `copy_area` per box.

- **yserver:** `render_composite` / `render_composite_via_frame_builder`
  (`crates/yserver/src/kms/v2/engine.rs:6626,6697`) always build/use a cached
  composite pipeline and issue a shader-based draw. No shortcut routes an
  opaque/`PictOpSrc` matching-format Composite into the cheaper native
  `vkCmdCopyImage` path — which yserver **already has** for real CopyArea
  (`engine.rs:4013` → `emit_recorded_copy_area_into_cb`, `engine.rs:8893`).
- **Xorg/glamor:** `glamor_composite_clipped_region` detects Composite calls that
  are mathematically equivalent to a copy (no mask, no transform, matching depth,
  `PictOpSrc` matching-format or `PictOpOver` opaque-source) and redirects straight
  into `glamor_copy()`, skipping shader/blend/texture-bind
  (`glamor/glamor_render.c:1539-1570`).
- **Note:** yserver's CopyArea is *more* efficient per-op than glamor's (native blit
  vs shader-sampled quad) — it just never routes eligible Composites into it.

### 4. Cache the ClipByChildren computation
- **yserver:** the `ClipByChildren` child-window subtraction
  (`crates/yserver/src/kms/v2/backend.rs:2618-2653`, also `7326-7337`) iterates the
  **entire** `self.windows_v2` map (all windows in the server), filtering for mapped
  children of the target, **on every single paint request** into a container window.
  No serial/generation gate against "has this window's child set changed."
  O(total-windows) per op.
- **Xorg:** the composite clip (`clipList`/`borderClip`) is computed once per
  structural change in `ValidateTree` (which bumps `drawable->serialNumber`) and
  cached on the GC; `VALIDATE_DRAWABLE_AND_GC` (`include/dix.h:103-115`) only
  recomputes when `pGC->serialNumber != pDraw->serialNumber`. A stream of paints
  between structural changes pays the clip cost once.
- **Relevance:** ties directly to the historical tray-storm bug class.

### 5. Multi-region CopyArea batching
- **yserver:** when clip-splitting produces multiple disjoint destination sub-rects
  for one CopyArea, each sub-rect becomes its own `engine.copy_area()` call → its own
  `RecordedCopyArea` op → its own **pair of `vkCmdPipelineBarrier2` calls** plus a
  single-region `vkCmdCopyImage` (`crates/yserver/src/kms/v2/engine.rs:8893-9070`;
  the N-separate-call pattern is pinned by tests at `backend.rs:22237-22246`).
- **Xorg/glamor:** `glamor_copy_fbo_fbo_draw` draws **all** boxes of a multi-box
  clipped copy in one VBO + one draw call (`glamor/glamor_copy.c:345-493`).
- **Fix shape:** `vkCmdCopyImage` natively accepts an array of regions, so N sub-rects
  could share one barrier pair + one multi-region copy. Narrow but real (currently
  N barrier pairs where 1 suffices).

---

## Tier 3 — high impact but structural / already tracked / blocked

### 6. Re-enable buffer-age partial repaint  (= make-v2-fast Task 4)
> ⛔ **DEAD (2026-07-08) — no measured cost, no safe workload, blocked correctness bug is inert.**
> Once believed "the single biggest structural cost," but measurement retired that: Always-Full
> has **no observable cost** on dev HW, and every workload is either smooth without it (mpv /
> chromium+YT on non-composited fvwm; cinnamon dual fullscreen 100–119 Hz) or a case it can't
> help (composited = COW re-presents full every frame). The one regime it could help + be safe
> (non-composited) is now proven smooth without it. The damage-completeness bug that blocks it
> (stale peek-through even on a STATIC window; `output_damage` under-reports) is **latent** —
> it only manifests if the disabled clipped path is re-enabled, which nothing needs. Could be
> revived only if some future real workload makes full-output recompose actually cost (weak GPU
> at high res, power/battery) — speculative, zero evidence today. See
> [[reference_buffer_age_dead_end_composited]] + [[reference_fvwm_slow_use_e27]].

- **Code state:** `pick_repaint_region` (`crates/yserver/src/kms/v2/scene.rs:1697-1746`) is
  hard-overridden to `Repaint::Full`; the `BufferAgeRing` (`scene.rs:259-312`) +
  `Repaint::Clipped(rect)` (`loadOp=LOAD`) machinery is built but disabled. Re-enable attempts:
  branch `perf/reenable-buffer-age` (f52796d1), shelved twice.

### 7. Direct-scanout flip for fullscreen unredirected windows  (= make-v2-fast Task 7)
> ⛔ **SHELVED — and the motivating symptom turned out to be an INSTRUMENTATION ARTEFACT.**
> #7 requires an *unredirected* output-covering window; `participating=0` under the instrumented
> **compositing** WMs (awesome/i3+comp/e27/cinnamon) means nothing to flip there. It was scoped
> off "choppy fullscreen video under fvwm" — but that symptom was a per-frame `scene`-debug log
> on a deleted branch starving the loop (observer effect). Re-tested on master with
> `just yserver-fvwm3-hw-telemetry`: chromium+YT fullscreen under **non-composited fvwm is
> smooth, zero drops**. So there is **no perf need for #7 on any tested config** (cinnamon also
> 100–119 Hz zero drops). M1 (RX 580 imports a client dma-buf as a scanout FB) proven but with
> no symptom to justify wiring it in. Full write-up:
> `docs/superpowers/plans/2026-07-08-direct-scanout-fullscreen-scope.md` (SHELVED header)
> + `findings/2026-07-08-perf-thread-wm-redirect-model.md`.

- **yserver:** the flip-vs-copy selector exists and is Xorg-faithful —
  `present_scheduler.rs:139-168` (`choose_path`) computes `DirectScanout` / `Flip` /
  `Copy` from scanout-compat + fullscreen-exact + full-region predicates (unit-tested
  at `present_scheduler.rs:372-423`). **But its result is discarded:**
  `crates/yserver-core/src/core_loop/process_request.rs:7928-7937` computes the path
  only as enqueued metadata — the comment states the synchronous `copy_area` above is
  what produced the pixels "regardless of path." Independently, the KMS
  `SceneCompositor` (`scene.rs::tick_one_output`, `scene.rs:1376-1695`)
  **unconditionally** calls `record_compose_v2` (the full Vulkan blit) for every
  dirty output — there is no fullscreen-single-window direct-flip short-circuit
  anywhere.
- **Xorg:** `present_check_flip` (`present/present_scmd.c:58-153`) page-flips a
  fullscreen unobscured unredirected window at **zero GPU cost** (window pixmap ==
  screen pixmap, `clipList == root winSize`, dims match) instead of compositing.
- **Relevance:** this is directly the issue-#82 fullscreen-wallpaper /
  maximized-video / single-terminal case. See
  `project_yserver_compose_responsiveness` memory.

### 8. Pixmap pooling >256px + EXA-style residency  (= make-v2-fast Task 5)
- **Pool coverage stops at 256px, exact-size keys, no eviction.** `PixmapPool`
  (`crates/yserver/src/kms/vk/pixmap_pool.rs`) is a `HashMap` keyed on exact
  `(w, h, format)`, capped 32/bucket, gated to `w,h <= 256`. Above that — most real
  window content and **every** full-output compositor backing — `allocate_drawable_storage`
  (`crates/yserver/src/kms/v2/platform.rs:1720-1791`) does a fresh
  `vkCreateImage`+`vkAllocateMemory`+`vkBindImageMemory`+`vkCreateImageView` with a
  bare first-fit memory-type scan and no cross-drawable suballocation. Xorg's
  `exaOffscreenAlloc` (`exa/exa_offscreen.c:161-260`) is one shared heap with
  first-fit + cost-based eviction (`size / age`) + adjacent-free coalescing, applied
  uniformly to any size.
- **No CPU/GPU residency score.** Every drawable is unconditionally `DEVICE_LOCAL`,
  and `put_image` (`engine.rs:5201-5320`) *always* stages + GPU-copies, regardless of
  how frequently the drawable is CPU-touched. Xorg's EXA migration score
  (`exa/exa_migration_classic.c:459-528`, `exa_priv.h:268-273`) keeps a hot-PutImage
  pixmap system-resident and services it via `memcpy` — no GPU round-trip. glamor
  additionally keeps small/glyph/scratch pixmaps CPU-side entirely
  (`glamor/glamor.c:202-224`), creating the GPU texture only lazily on first
  accelerated use.
- **Note:** the staging-buffer pool (`fc31633e`) is the one axis where yserver is
  *ahead* of glamor (which allocates a fresh PBO per fallback access). The remaining
  gap there is that `acquire_upload_staging` keys on **exact** size
  (`engine.rs:1213`), so partial-window PutImage rects mostly miss the pool — a
  size-bucketed/rounded key would raise the hit rate.

---

## Appendix A — "Task 3 submit-aggregation" was a phantom

Context, since this comparison grew out of re-examining that task. The
handoff-recorded motivation — *"flooder is submit-bound: ~3096 submits/s @ 99.8%
batch_size=1, coalesce them"* — was a **misread of the trace file**:

- The `*.submit.tsv` rows labeled `put_image batch_size=1` come from
  `trace_simple(SubmitKind::PutImage, …)` (`backend.rs:14601`), which fires **once per
  PutImage request** with a **hardcoded `batch_size: 1`**. It is a per-*request*
  marker, not a `vkQueueSubmit2`.
- `engine.put_image` only **appends to the FrameBuilder open frame**
  (`engine.rs:5201`); the real submit happens later at frame **close**.
- Real close telemetry from the same run: `frame_builder closes=20/s,
  ops/frame_avg=1.6, close_reasons[timeout=17 present_completion=3]` → ~20–60 real
  submits/s, **already batched**.
- This is why "raising `SubmitGroup.max_size` didn't fix it": the FrameBuilder
  already collapses a frame's ops into one CB. `max_size=1` (Phase B Invariant M1,
  `submit_group.rs:71`, recovers at B.5) only groups *multiple* CBs per compose tick,
  and a single-target flood produces one CB per tick anyway.
- The make-v2-fast doc's Task 3 (2026-05-20) predates the FrameBuilder, is broader
  (all paint ops), landed its load-bearing parts (CopyArea `0bec1b3`,
  render_composite `68af625`), and **explicitly deprioritized PutImage** ("~8% of
  submits, not the hotspot"). Its one genuinely-open submit item is **B.5** (retire
  the M1 cap) — which only pays off on a real multi-CB workload (silence/bee MATE
  drag at the doc's measured 2119 *real* `queue_submit2`/s), not the single-target
  flooder.

The flood itself was already resolved by the staging-buffer pool (`fc31633e`).

## Appendix B — already at parity or ahead (do not re-investigate)

Verified present and comparable-or-better in yserver v2:

- **Staging/upload buffer pool** — fence-gated persistent free-list
  (`engine.rs:1192-1230`); glamor has no pooled upload path.
- **Pixmap-backing recycling** (≤256px) — `pixmap_pool.rs`; no glamor counterpart.
- **Glyph atlas** — shelf packer + hashmap (`glyph_atlas.rs`); parity with glamor
  (both lack LRU eviction).
- **Shader/pipeline caching** — `RenderPipelineCache` keyed on `(PictOp, Format,
  bool, bool)` (`vk/render_pipeline.rs:306-420`) + `text_pipelines`; structurally
  equivalent to glamor's shader arrays.
- **Deferred-submit batching** — FrameBuilder is a stronger, explicit analogue of
  glamor's implicit GL-driver batching.
- **Solid-fill fast path** — native `vkCmdClearAttachments` (`engine.rs:9662-9691`),
  no shader; stronger than glamor's shader-based solid fill.
- **Gradient caching** — LUT/image rendered once at
  `RenderCreate{Linear,Radial}Gradient` time and `Arc`-shared (`vk/gradient.rs:83-100`);
  glamor re-renders per Composite.
- **SolidFill source fast path** — `PictureRecord::SolidFill` →
  `ResolvedSource::Solid` (`backend.rs:10124-10133`); parity with
  `XRenderCreateSolidFill` (the missing variant is 1×1-repeat *drawable*-as-solid,
  see below).
- **Trapezoid/triangle accumulate-then-composite** — one instanced draw into
  `mask_scratch` (`engine.rs:10405`); matches glamor.
- **Redirected backing-pixmap reuse on resize** — `redirected_backing_can_fit`
  (`backend.rs:12826-12856`) wired in production (`process_request.rs:980-1030`);
  parity with Xorg `compReallocPixmap`.
- **Damage report-level semantics** — RAW/DELTA/BOUNDING_BOX/NON_EMPTY
  (`yserver-core/src/core_loop/damage_fanout.rs:623-629`) match Xorg's four levels.
  (Gaps: DELTA dedup not implemented, O(D) lookup, `RegionSet` lacks true region
  algebra — lower real-world impact today.)

Non-gaps confirmed against the *current* Xorg tree (historical features that were
removed upstream, so not worth adding): DIX scratch-pixmap reuse (removed,
`dix/pixmap.c:44-53`), glamor FBO/texture cache (none in this tree), glamor
large-pixmap tiling (a GL texture-size workaround, irrelevant at Vulkan's 16384
limit).

## Recommended next work

**Status 2026-07-08:** Tier 1 shipped (#1 `ae5f6bc7`, #2 `c35ac33f`). #6 and #7 are DEAD
(no measured need — see their entries). Remaining live items are #3/#4/#5/#8.

**Caveat learned this session:** unlike #1/#2 (clear "GTK/Pango text is the heaviest workload"
rationale), #3/#4/#5/#8 have **no profile proving they're bottlenecks** on a real workload.
Scoping an optimization off an unmeasured symptom is exactly what sent us chasing the
buffer-age/fvwm dead-end (the fvwm "choppy" was an instrumentation artefact). Prefer to
profile a real busy desktop before implementing, OR pick items that are correct-improvements
regardless of a measurement.

1. **#3 Composite→CopyArea fast path** — ✅ **DONE** (clip-aware, HW-validated on mate). See
   the entry above. Follow-up: fold in #5 (multi-region copy) so each clipped composite is one
   `vkCmdCopyImage` over N boxes rather than N `copy_area` calls.
2. **#4 ClipByChildren caching** — algorithmic `O(total-windows)`-per-paint fix + correctness-
   adjacent (tray-storm class); worth doing regardless of a profile. **← next.**
3. **#5 multi-region CopyArea batching**, **#8 pixmap pooling >256px + EXA residency** —
   larger; #8 is the biggest remaining structural item. Profile-gated. (#5 now also wanted by #3.)
