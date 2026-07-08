# Scope: glyph draw batching (#1) + A1→A8 expansion caching (#2)

**Date:** 2026-07-08
**Source:** `docs/superpowers/findings/2026-07-08-xorg-render-optimization-gaps.md` Tier 1.
**Status:** #2 **DONE** (shipped to master `c35ac33f`, 2026-07-08 — HW-confirmed
wmaker: st+Terminus A1 upright, xterm/wezterm/gkrellm A8 no-regression). #1 scoped,
not started.

Both items target the heaviest real RENDER workload (GTK/Pango + terminal text).
They are independent; #2 was the smaller quick win and landed first.

---

## Shared context: the two glyph paths

- **Wire parse** — `render_composite_glyphs` (`backend.rs:~15870-16090`) decodes the
  `CompositeGlyphs` request into `Parsed` → `CompositeGlyphInput<'_>` and calls
  `engine.composite_glyphs(...)`.
- **Atlas intern + record** — `engine.composite_glyphs` (`engine.rs:~6244` walk) dedups
  each glyph against (a) committed atlas `lookup(key)`, (b) open-frame pending inserts,
  (c) this-call `new_uploads`. **Only a full miss reads `g.pixels` and uploads.**
- **GPU record** — both `composite_glyphs` (scissored, `engine.rs:8562`) and `image_text`
  (`engine.rs:10026`, non-scissored `record_text_run`) funnel into
  `record_text_run_scissored` (`vk/ops/text.rs:110`). This is the per-glyph-draw loop.

The **template for #1 already ships**: the trapezoid path packs N traps into a
per-instance vertex buffer (`TrapInstanceData`, 40B, `VK_VERTEX_INPUT_RATE_INSTANCE`,
`trap_pipeline.rs:46/294`), binds it from a pinned staging buffer
(`engine.rs:10299 pins.staging_buffers[rt.vertex_pool_pin.0]`), and issues one
`cmd_draw(cb, 4, instance_count, 0, 0)` per scissor (`engine.rs:10329`).

---

## Item #2 — Gate A1→A8 expansion on cache-miss  ✅ DONE (`c35ac33f`)

Implemented as scoped below. New `kms::v2::glyph_pixels` module
(`GlyphPixels{A8,A1Wire}` + `to_a8` + relocated `expand_a1_glyph_to_a8` and its
#77 tests); `CompositeGlyphInput.pixels: &[u8]` → `GlyphPixels`; backend forwards
raw glyphset bytes (no more per-call `a1_scratches`); engine expands A1 only in the
atlas-miss branch. A8 path is an unchanged zero-copy borrow; core ImageText untouched.

### Problem
`expand_a1_glyph_to_a8` (`backend.rs:18603`) runs **unconditionally in the wire parse**
(`backend.rs:15965`) for every A1 glyph in every request, into a per-call `a1_scratches`
Vec. But the expanded bytes are only ever *read* on a cache miss
(`engine.rs:~6330`, the staging `copy_nonoverlapping`). A glyph already resident in the
atlas pays a full CPU bit-unpack every frame it appears and the result is discarded.
ARGB32 glyphs are already converted once at ingest (`parse_add_glyphs`); A1 is the straggler.

### Fix shape (defer expansion into the engine miss-branch)
1. Move `expand_a1_glyph_to_a8` to a shared location reachable from both backend and
   engine (it's a free fn today, used only in backend + its tests). Keep the #77 LSBFirst
   semantics and the existing regression test (`a1_glyph_expands_lsb_first_not_mirrored`).
2. Change `CompositeGlyphInput.pixels` from a pre-expanded `&[u8]` to a small enum that
   carries the source as-is:
   `A8(&[u8])` (glyphset slice, unchanged) | `A1Wire(&[u8])` (raw wire bytes + the
   `w,h` already on the struct give the stride).
3. Backend stops eager-expanding: drop `a1_scratches`, `PixelSource::A1Scratch`, and the
   `A1` arm just forwards the wire slice. Pass 2 (`backend.rs:16045`) forwards the enum.
4. Engine miss-branch only: expand `A1Wire` → A8 (`w*h` bytes) into the staging buffer;
   `A8` copies directly as today. **Nothing changes on a hit** — pixels are never touched.
5. Make the length pre-validation format-aware: A8 needs `len >= w*h`; A1 needs
   `len >= div_ceil(w,32)*4*h` (`engine.rs:6304-6316`).

### Blast radius / risk
- Types crossing the backend↔engine seam (`CompositeGlyphInput`) — contained, one caller.
- Correctness guarded by the existing LSBFirst test; add a test that a **repeated** A1
  glyph expands **once** (assert the expander runs on miss only — e.g. via an intern/upload
  counter, since `stats.glyph_uploads`/`atlas_interns` already exist).
- Risk: **low**. No GPU/pipeline/shader changes. No blend semantics touched.
- Effort: **~half day.**

---

## Item #1 — Batch glyph draws into one instanced draw per run  (top pick, medium)

### Problem
`record_text_run_scissored` (`text.rs:189-221`) nests
`for scissor { set_scissor; for glyph { push_constants; cmd_draw(4,1,0,0) } }` — one
push-constants + one draw **per glyph per clip-rect**. The pipeline binds no vertex buffer
(`text_pipeline.rs:238`), driven by `gl_VertexIndex`; per-glyph rect/UV ride in push
constants (`TextPushConsts`, `text.vert.glsl`). Dense text = N command-buffer entries +
N driver draw-state validations where **one instanced draw** suffices.

### Fix shape (mirror the trapezoid instanced path)
1. **New GPU struct** `GlyphInstanceData` (mirror `TrapInstanceData`): per-instance
   `dst_origin: vec2, dst_size: vec2` + **atlas TEXEL coords** `atlas_xy: vec2,
   atlas_wh: vec2` (NOT normalized UVs — see codex #1) = 32B,
   `VK_VERTEX_INPUT_RATE_INSTANCE`, split into vertex attributes. `size_of == 32` assert.
   These texel values are exactly what `RecordedTextGlyph` already stores, so record-time
   just copies them — no normalization at record time.
2. **Split the push constants**: `viewport`, `foreground`, **and `atlas_extent`** are
   per-*run* constants → stay in push constants (reshaped `TextPushConsts`). Per-glyph
   dst rect + atlas texel coords become per-instance attributes. Fix the offset/size
   asserts (`text_pipeline.rs:101-102`).
3. **Rewrite `text.vert.glsl`**: read the four per-instance `vec2` attributes; compute
   the UV as `atlas_xy / atlas_extent` and `atlas_wh / atlas_extent` in the shader
   (normalization stays at draw time, reading the *current* atlas extent via push
   constant — preserves today's behavior and is safe if a future Stage-5 atlas
   grow/repack lands). Keep the `gl_VertexIndex` quad + `layout(scalar)` note.
   `text.frag.glsl` unchanged (samples atlas, applies foreground; A8_DST spec const stays).
4. **Pipeline**: add the `PipelineVertexInputStateCreateInfo` (binding 0 INSTANCE-rate +
   4 vec2 attributes, 32B stride) — currently `default()`. This is the one behavioural
   change to the text pipeline; the `(op, dst_format, dst_has_alpha)` cache key and blend
   derivation are untouched.
5. **Recorder**: `record_text_run_scissored` builds a `Vec<GlyphInstanceData>` once (skip
   `w==0||h==0` as today), uploads it to a staging buffer, binds it as the instance vertex
   buffer, then per scissor: `set_scissor; cmd_draw(4, n, 0, 0)`. One push-constants for
   the run. **The instance buffer is identical across scissors** — build once, one draw
   per rect. **Zero-instance fast path** (codex #5): if the filtered run is empty, skip
   buffer creation/pin and all draws entirely — don't pin a useless buffer.

### Instance-buffer lifetime — RESOLVED (codex-confirmed)
Both call sites — `Op::CompositeGlyphs` (`engine.rs:8566`) and `Op::ImageText`
(`engine.rs:10026`) — call the recorder at **FrameBuilder EMIT time into the deferred
frame `cb`**, not an immediate CB. So a transient staging buffer freed on fence would die
before the deferred submit. **Pin at RECORD time exactly like traps' `vertex_pool_pin`:**
build+fill+`open.pins.pin_staging(Arc::new(instance_buf))` when the
`RecordedCompositeGlyphs`/`RecordedImageText` payload is built (where the atlas entries +
`RecordedTextGlyph` list already are), store a `PinnedStagingIdx` on the payload, bind at
emit. Same caveat as traps: everything the payload references must outlive submit.

### Blast radius / risk
- `text_pipeline.rs` (vertex input + push-const layout + asserts), `text.vert.glsl`,
  `vk/ops/text.rs` (both `record_text_run` and `_scissored`), instance-buffer plumbing at
  both engine call sites.
- Correctness watch-items: (a) a8-mask / component-alpha dst (`R8_UNORM`, `op=Add`)
  renders identically — codex #4: holds because `Add` is saturating/commutative, so one
  instanced draw == N sequential over overlapping quads; (b) for non-additive ops,
  equivalence relies on **preserving glyph order** within the instance array (treat as an
  explicit invariant, not incidental); (c) per-run `foreground` still applies to every
  glyph; (d) empty-glyph skip preserved + zero-instance fast path; (e) scissor still clips.
- ABI (codex #6): after the split, revalidate `#[repr(C)]` ↔ `layout(scalar)` offsets AND
  the new vertex-attribute offsets/stride — silent-failure class (the historical
  "black→green text" `offset_of!` assert guards the push-const half; add analogous
  coverage for the attribute layout).
- Risk: **medium** — GPU pipeline + shader change on a hot, correctness-sensitive path.
  Mitigate with golden-image parity tests (existing text render tests) before HW.
- Effort: **~1–2 days**.

### Codex review verdict (2026-07-08)
Scope judged sound; no blocking issues for the current atlas. Findings folded in above:
texel-coords-not-UV + atlas-extent push constant (#1, latent Stage-5 trap — atlas is a
fixed 4096² image today, verified, so not a live bug but the cleaner design anyway);
record-time pin confirmed correct (#2); instanced-vertex-attrs preferred over SSBO for
32B/glyph (#3); R8 `Add` equivalence holds (#4); zero-instance fast path (#5); ABI
revalidation (#6); scissor semantics preserved (#7).

---

## Recommended order
1. ~~**#2 first** — small, isolated, no shader/pipeline risk; de-risks the seam types.~~
   ✅ done (`c35ac33f`).
2. **#1** (next) — resolve the instance-buffer-lifetime question, then mirror the trap path.
3. Both gate on: `cargo fmt` + `clippy -W pedantic` + `cargo test` + **HW smoke** (text is
   render-path; vng/lavapipe won't catch a blend/UV regression — needs silence/bee/eiger).
4. Per CLAUDE.md: **codex review of this scope + the eventual plan** before implementation.
