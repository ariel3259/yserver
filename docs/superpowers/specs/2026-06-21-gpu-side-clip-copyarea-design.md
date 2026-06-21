# GPU-side clip for clip-masked CopyArea — design

**Status:** draft, 2026-06-21. Branch `diag/gkrellm-syncboundary-attribution`
(follows the merged-pending readback-fix work + the copy_area attribution
commits `b2a92056`, `8d15d1ab`). Direction codex-GO'd (gpt-5.4, 2026-06-21):
GPU-side clip for CopyArea first, sampled-mask not stencil, GXcopy-only.

**One-line framing:** a clip-masked GXcopy CopyArea is the gkrellm CPU sink
(op62 ~750/s, ~280ms/s on bee, `cpu_fence_wait≈0`). It fans out to ~4–6 GPU
blits per copy (~82% from clip-mask runs, measured). Replace the
CPU-mask-run-extraction + N-transfer-copy path with **one sampled graphics
draw** that binds the clip-mask pixmap's existing depth-1 GPU image as a mask
and discards masked-out fragments — killing the CPU run extraction, the per-run
transfer fan-out, AND the clip-mask `get_image` readback in one move.

## Measured justification

Bee gkrellm/cinnamon + 6-core vng, after the readback fix:
- `op62` (CopyArea) = `req_time`'s dominant cost (~280ms/s bee, `fence≈0` →
  CPU-bound in the handler, not GPU waits).
- `op62 ≈ copy_area_calls` (1:1, real client copies) → each fans out to
  `copy_area_gpu_subrects ≈ 4–6 ×` calls.
- Split (`b2a92056`): **maskrun ~82%** (`ClipState::Pixmap` clip-mask-run path,
  backend.rs:12694) vs rectclip ~18% (GC-Rectangles/ClipByChildren,
  backend.rs:12933). So the fan-out is overwhelmingly clip-mask runs.

The per-copy cost is: (1) `intersect_with_current_clip_live` rasterizes the
depth-1 mask into ~4 rectangular runs on the CPU, plus (2) N×`cmd_copy_image`
transfer blits + CB records, plus (3) the residual `get_image` clip readback
(content-driven, ~175/s) feeding the CPU rasterization. GPU-side clip removes
all three.

## Current path (what we're replacing)

`backend.rs::copy_area` (12551), `ClipState::Pixmap` branch (12642):
1. `intersect_with_current_clip_live(&[dst_rect])` → `Vec<run>` (CPU rasterize
   the cached mask bytes into rectangles; the cache is the FIX-A snapshot, but
   the rasterization itself is CPU per-op).
2. For each run: `engine.copy_area` / `cow_copy_area` → recorded as
   `RecordedOp::CopyArea`, emitted at frame close as `cmd_copy_image`
   (engine.rs:7712 `emit_recorded_copy_area_into_cb`) — a **transfer copy**, no
   shader, so it cannot sample a mask.

CopyArea is therefore a transfer today; GPU-side clip requires a **graphics
draw**.

## Proposed path: sampled masked-blit draw

For a clip-masked, full-plane-mask, GXcopy CopyArea where the clip is
`ClipState::Pixmap`:

- Issue **one graphics draw** over the copy's dst rect (intersected with the
  GC-rect / child / window clip via **scissor** — see § Rect-clip composition).
- The fragment shader samples **src** at the copy offset (NEAREST) and the
  **clip mask** at `(dst_frag − clip_origin)`, and **discards** where the mask
  texel is zero; otherwise writes the src texel **verbatim** (no blend, no
  alpha/format munging — GXcopy is a raw copy).
- The mask is sampled from a **GPU snapshot** of the clip pixmap (see § Mask
  source), NOT the live drawable image — **no `get_image` readback, no CPU
  rasterization.**

### Pipeline: a dedicated masked-blit pipeline (texelFetch)

**Decision (codex review):** do NOT reuse the RENDER pipeline. `render.frag`
samples via normalized UVs (`texture()`) and carries premultiply /
PictFormat / force-opaque / component-alpha machinery — the wrong foundation
for "must equal `cmd_copy_image` byte-for-byte". Add a small **dedicated
`masked_blit` pipeline**:
- **`texelFetch`** (integer texel addressing) for BOTH src and mask — exact
  texel selection, no filtering, no UV rounding.
- sample `mask.r`; `discard` if `== 0` (hard 1-bit clip threshold, NOT alpha
  coverage — see below).
- `discard` if the projected **src texel is out of src bounds** (see §
  Geometry) — never write a "sampled-zero" texel.
- write the src texel **verbatim**: blend disabled, no alpha/PictFormat
  transform, no force-opaque. For depth-24 BGRA the unused X/alpha byte is
  copied exactly as `cmd_copy_image` would (raw bytes), not synthesized.

It reuses the frame builder's draw-recording machinery and the dynamic-scissor
mechanism (engine.rs:7621) but has its own minimal descriptor layout
(src image, mask image) and push constants (src offset, mask origin, rects).

**Image-view contract (codex blocker).** `Storage` has TWO views (store.rs:93):
`image_view` (IDENTITY swizzle) and `sample_view` (format/depth-aware swizzle —
depth-24 BGRA pins `α=ONE`, `R8_UNORM` maps `R→A`). `masked_blit` MUST bind the
**IDENTITY `image_view`** for BOTH src and mask, NOT `sample_view`, because:
- the mask bit lives in the real **R** channel — the R8 `sample_view`'s `R→A`
  swizzle would move it to `.a`, so `texelFetch(mask).r` only reads the bit on
  the identity view;
- depth-24 BGRA must be copied **raw** (incl. the X byte) — `sample_view`'s
  forced `α=ONE` would corrupt it.
`texelFetch` fixes coordinate exactness; binding the identity view fixes
channel/byte exactness. The snapshot image likewise exposes an identity R8 view.

### The depth-1 mask is a THRESHOLD, not alpha coverage

Load-bearing correctness point. `decode_x11_pixel_for_storage` stores a depth-1
"set" bit as the R8 byte **`0x01`** (not `0xFF`) — `pack_from_storage` reads
depth-1 back as `raw != 0`. So sampling the mask as normalized alpha gives
`0x01/255 ≈ 0.004`, which as RENDER coverage would nearly zero the source. The
CLIP_BLIT mode therefore **thresholds**: `mask.r != 0 → keep, == 0 → discard`.
It must NOT multiply src by mask coverage. (This is exactly the non-Copy R8
hazard codex flagged, in mask form: the byte value is a flag, not a fraction.)
The mask is read with `texelFetch` at integer coord `(dst - clip_origin)`; a
coord outside the mask image bounds is treated as **clipped (discard)**,
matching the CPU `intersect_with_current_clip` out-of-bounds rule.

### Mask source: a pinned GPU snapshot (retain-after-free + ordering)

X11 `XSetClipMask` snapshots the bitmap into the GC: freeing the source pixmap
afterward MUST NOT change clipping (backend.rs:6015/6136 + the FIX-A frozen
snapshot enforce this today). A live `mask_tex` lookup cannot satisfy that, and
sampling the live pixmap also races a same-frame write to it. So the masked-blit
samples a **GC-owned GPU snapshot** of the mask:

- At clip-mask install / first masked use, snapshot the clip pixmap's depth-1
  image into a dedicated, **pinned** `R8_UNORM` image owned by the clip state,
  keyed by `(mask DrawableId, content_version)`.
- Reuse the snapshot across every copy through that mask; **re-snapshot only
  when `content_version` changes** (the FIX-A primitive) - a static mask
  snapshots once; gkrellm's per-frame-rewritten mask snapshots ~once/frame,
  amortized over the ~N copies in the frame, NOT per copy.
- The snapshot **persists after the source pixmap is freed** (it is our image),
  giving retain-after-free for free.
- The snapshot copy is itself a frame op that **reads the LIVE clip pixmap**, so
  on the refresh path the live mask drawable is a first-class participant: its
  own append-time `first_touch`, `last_render_ticket` touch, and recorded
  `live_mask_old_layout`, with the copy ordered after any same-frame write to it
  and the masked-blit draw ordered after the snapshot — all via explicit
  barriers (see § Frame-builder integration). On the **cache-hit path** (version
  unchanged) there is no live-mask access at all: the masked-blit samples the
  existing snapshot, so a freed source pixmap is never touched.

This replaces the FIX-A *CPU-bytes* cache on the CopyArea path with a *GPU-image*
snapshot keyed by the same `content_version`. FIX-A's CPU cache stays for the
still-CPU-clipped ops (fills/segments) until they too move to GPU clip.

### Geometry: clamp + project exactly like the transfer path

`engine.copy_area` clamps `src_rect` to src bounds and projects/clamps the dst
rect (engine.rs:3263), handling X11 negative offsets. The masked-blit MUST
record the **already-clamped effective `src_rect` and `dst_rect`** (reuse that
arithmetic verbatim) and draw only over the clamped dst rect; the shader
additionally `discard`s any fragment whose projected src texel is OOB. It must
never rely on "sample zero outside" - a sampled-zero write would corrupt dst
where the transfer path would have left it untouched.

### Exactness gate (GXcopy = byte-exact)

`cmd_copy_image` is a raw byte copy. A graphics draw round-trips through format
interpretation (UNORM normalize, shader float, write). With `texelFetch` (no
filtering) and blend disabled this is exact, but it MUST be proven. The lavapipe
equivalence test (§ Tests) compares **raw storage bytes** of the kept region to
`cmd_copy_image`'s result — **including the unused X/alpha byte for depth-24
BGRA**: yserver forces server-owned alpha for depth<32 on direct *writes*
(engine.rs:9318/9345), but a GXcopy must copy the source bytes as-is, so the
dedicated masked-blit pipeline inherits NO alpha-fixup path. If any format fails
byte-exactness, that format stays on the transfer path (scoped out, not shipped
wrong).

### Self-overlap (src == dst)

A draw cannot sample `dst_tex`/`src_tex` while writing the same image. The
current path already allocates `self_overlap_scratch` (engine.rs:3302) for
`src == dst`. The masked-blit reuses it: copy the src region into scratch
first, sample scratch as `src_tex`. (Window self-scroll — Tk text widgets,
gkrellm chart scroll — is exactly this case, so it must be correct.)

### Rect-clip composition (the other ~18%, and window/child clip)

The mask sampler handles the `ClipState::Pixmap` bitmap. The GC-rectangle clip,
`ClipByChildren` child subtraction, and window-bounds clip remain rectangle
sets (backend.rs:12743-12933). The masked draw applies them as **one masked
draw per surviving rect, each with a single dynamic scissor** — matching the
existing per-rect-scissor machinery (engine.rs:7621), not a scissor-array (the
codebase sets one scissor at a time). For the common gkrellm case there is one
dst rect, so it is one scissored draw + the mask sample. Pure rect-clip copies
with no Pixmap mask (the 18%) stay on the transfer path initially; folding them
into the draw is a follow-up, not required for the gkrellm win.

## Scope (first cut)

Terminology: the **clip mask** is always a **depth-1 bitmap** (the
`ClipState::Pixmap`), sampled as a threshold. The **src/dst format** is the
copy's own format (BGRA8 depth-32/24, R8 depth-8) — that is what the exactness
gate covers.

IN: `CopyArea` where the GC clip is `ClipState::Pixmap` (depth-1 mask) +
`function == GXcopy` + full plane mask + a **src/dst format that passes the
exactness gate** (BGRA8 depth-32/24, R8 depth-8).

OUT (stay on the existing transfer path; all cold per telemetry):
- non-Copy rops (GXxor etc.) — boolean R8 hazard, separate work.
- partial plane masks.
- non-depth-1 clip masks (a clip mask is depth-1 by X11 definition; defensive).
- any src/dst format that fails the byte-exactness gate.
- the rect-clip-only / ClipByChildren fan-out (18%) — optional follow-up.
- generalizing GPU-side clip to fills / segments / glyphs — a later phase
  (CopyArea is the measured hotspot and the proving ground).

## Frame-builder integration

Add a `RecordedOp::MaskedCopyArea` variant holding: src/dst `DrawableId`s, the
mask **snapshot** image handle (§ Mask source), clamped src offset + dst rect,
clip origin, the surviving scissor rects, recorded `src_old_layout` /
`mask_old_layout` / `dst_old_layout`, and the self-overlap scratch (if any).

**Three drawable roles must be handled at append (codex):**
- **dst:** first-touch into the open frame, layout-overlay update, damage, and
  the `content_version` bump (MaskedCopyArea is a write — the existing 8-writer
  discipline).
- **src:** first-touch, `last_render_ticket` touch, recorded `src_old_layout`.
- **mask snapshot:** first-touch, `last_render_ticket` touch, recorded
  `mask_old_layout`; its lifetime is the GC-owned pinned snapshot (NOT the live
  clip pixmap), so it survives the source pixmap being freed. The masked-blit
  draw samples THIS.
- **live mask source — ONLY on the refresh path** (when `content_version`
  changed, so a re-snapshot is emitted): the live clip pixmap drawable also
  gets first-touch, `last_render_ticket` touch, and a recorded
  `live_mask_old_layout`, because the snapshot-copy op reads it. The snapshot
  copy is ordered after any same-frame write to the live mask and before the
  masked-blit. On the cache-hit path this role is absent entirely.

**Barriers / ordering (codex blocker — `content_version` is cache-invalidation,
NOT a GPU memory dependency).** The close-time emit MUST, like the other ops,
derive explicit `PipelineBarrier2` from the recorded old layouts: transition
src + mask snapshot to `SHADER_READ_ONLY_OPTIMAL` (`FRAGMENT_SHADER` /
`SHADER_READ`) and dst to `COLOR_ATTACHMENT_OPTIMAL` before the draw. When the
mask was snapshotted earlier the same frame (after a same-frame mask write), the
snapshot-copy op carries its own write→transfer→read barrier chain so the draw
samples the committed snapshot. Define the exact stage/access/layout contract
in the plan, mirroring `emit_recorded_copy_area_into_cb` (engine.rs:7651) and
the compose draw's barrier emission.

## What this eliminates (expected bee deltas)

- `copy_area_gpu_subrects` maskrun portion: ~4–6 blits/copy → **1 draw/copy**
  (≈ `copy_area_calls`).
- `op62` CPU: the ~280ms/s CPU mask-run rasterization + per-run CB recording →
  one draw record; expect a large drop in `req_time`.
- clip `get_image` readbacks on the copy path → **0** (mask sampled from its
  GPU image; no readback, no FIX-A cache lookup on this path).
- `sync_boundary` flushes from those readbacks → down.

## Correctness analysis

- **Byte-exactness** vs `cmd_copy_image`: gated by the lavapipe equivalence
  test per format; non-passing formats stay on the transfer path.
- **Mask threshold**: `!= 0` keep / `== 0` discard matches the X11 1-bit clip
  semantics and the `pack_from_storage` `raw != 0` convention; immune to the
  `0x01`-vs-`0xFF` representation.
- **Clip origin**: clip origin passed as a push constant; `texelFetch` at
  integer `(dst - clip_origin)`, no filtering — exact texel selection, matching
  the CPU `intersect_with_current_clip` rule (mask bit at `(dst - clip_origin)`,
  out-of-bounds = clipped/discard).
- **Geometry/OOB**: clamped effective src/dst rects recorded from
  `engine.copy_area`'s arithmetic; shader discards OOB-src fragments — never a
  sampled-zero write. dst outside the clamped rect is untouched.
- **Retain-after-free**: the mask is sampled from the GC-owned pinned GPU
  **snapshot**, not the live clip pixmap, so freeing the source pixmap after
  install does not change clipping (matches backend.rs:6015 + FIX-A).
- **Self-overlap**: scratch breaks the read-after-write; same guarantee as the
  current path.
- **Rect/child/window clip**: per-rect scissor reproduces the current rect
  intersection; no pixel outside the surviving rects is touched.
- **Mask write then sample, same frame**: correctness rides on **explicit
  PipelineBarrier2** from recorded old layouts (mask-write -> snapshot-copy ->
  masked-blit), NOT on `content_version` (which only gates re-snapshot /
  cache invalidation). The snapshot is taken after any same-frame mask write;
  the draw samples the snapshot after a SHADER_READ barrier.

## Tests

Lavapipe (`tests/v2_acceptance.rs`, `#[ignore]`):
- `v2_masked_copyarea_matches_cmd_copy_image` — for each in-scope src/dst format
  (BGRA8 depth-32, BGRA8 depth-24 **incl. the X/alpha byte**, R8 depth-8),
  masked draw output over the kept region is **byte-identical** (raw storage
  bytes) to `cmd_copy_image`; masked-out region untouched. THE exactness gate.
- `v2_masked_copyarea_honors_clip_origin` — non-zero clip origin selects the
  correct mask texels; mask-OOB region is clipped.
- `v2_masked_copyarea_clamps_negative_dst_and_oob_src` — negative dst offset +
  partially-OOB src: output matches the transfer path's clamp/project; no
  sampled-zero write outside the legal copy region.
- `v2_masked_copyarea_self_overlap` — `src == dst` overlapping scroll matches
  the CPU/transfer reference.
- `v2_masked_copyarea_scissor_composes_with_gc_rects` — GC-rect clip + mask
  together match the current run-based result.
- `v2_masked_copyarea_retains_clip_after_pixmap_freed` — install clip mask,
  FREE the source pixmap, then masked CopyArea still honors the snapshot.
- `v2_masked_copyarea_mask_written_same_frame` — fill the mask (GPU) then masked
  CopyArea through it in the SAME frame: result reflects the written mask (the
  barrier/snapshot ordering holds).

Crate-local (`for_tests_with_vk` + telemetry):
- `masked_copyarea_routes_to_draw_not_transfer` — a clip-masked GXcopy copy
  produces ONE draw and `copy_area_gpu_subrect[maskrun]` does NOT increment
  (replaced by the new path's counter); fan-out is 1/copy.
- `masked_copyarea_no_clip_get_image` — the copy path issues zero
  `GetImageSite::ClipMask` readbacks.
- `noncopy_or_partial_planemask_copy_still_uses_old_path` — scope guard.

Existing clip-correctness suite (`v2_clip_pixmap_mask_gates_poly_fill...`,
`render_composite_no_gc_clip_leak`, etc.) must stay green.

## Hardware / vng gate

bee + 6-core vng gkrellm, `YSERVER_LOOP_TELEMETRY=1`:
- `copy_area_gpu_subrect[maskrun]` → ~0 (replaced); new masked-draw count ≈
  `copy_area_calls` (1 draw/copy, no fan-out).
- `op62` `t/s` and `req_time%` drop sharply (the ~280ms/s mask-run cost gone).
- `get_image_by_site[clip]` on the copy path → 0.
- gkrellm core usage drops; no `DEVICE_LOST`; clip rendering visually correct
  (gkrellm charts, wmaker title buttons).

## Review trail / resolved decisions

Codex gpt-5.4 direction GO (2026-06-21); design review round 1 NO-GO (4
blockers) → revised; round 2 NO-GO (2 refinement blockers) → revised again;
**round 3 GO** (this doc). Round-2 resolutions: (a) the snapshot-refresh op records the LIVE
mask drawable as an explicit participant (first-touch/ticket/old-layout),
absent on the cache-hit path; (b) `masked_blit` binds the IDENTITY `image_view`
(not the swizzled `sample_view`) for src + mask, so the R8 mask bit is read raw
from `.r` and depth-24 bytes incl. the X byte are copied raw. Round-1
resolutions folded in:
- **Pipeline:** dedicated `masked_blit` with `texelFetch`, no RENDER
  premul/PictFormat — NOT a CLIP_BLIT mode on the render shader (blocker 3).
- **Retain-after-free:** sample a GC-owned pinned GPU snapshot, not the live
  clip pixmap (blocker 1).
- **Geometry:** record clamped src/dst rects + discard OOB-src; never
  sampled-zero-write (blocker 2).
- **Ordering:** explicit PipelineBarrier2 from recorded old layouts;
  `content_version` is cache-invalidation only (blocker 4).
- **3-role touch** (src/dst/mask) at append (should-fix 5).
- **Exactness:** raw-byte compare incl. depth-24 X byte; no alpha fixup
  (should-fix 6).
- **Rect-clip:** one draw per surviving rect, one dynamic scissor each
  (should-fix 7).
- **Scope:** CopyArea / GXcopy / depth-1-mask only; rect-clip-only stays on
  transfer (confirmed right).

Round 3: **GO** for writing the implementation plan (no remaining design
blocker). Two obligations the plan MUST close before implementation:
1. **Exactness gate is empirical** — run the lavapipe byte-equivalence test per
   src/dst format and EXCLUDE any that fail before shipping it (don't assume all
   three pass).
2. **Snapshot state carrier (codex round-3 MEDIUM).** The mask snapshot is a
   GC-owned pinned image, NOT a `DrawableId`; the existing transactional
   commit/rollback machinery handles only drawables + atlas (engine.rs:8784,
   store.rs:1102). The plan must define an explicit state carrier for the
   snapshot's `current_layout`, `last_render_ticket`, close-FAILURE rollback,
   and retirement/destruction after the GC (or its clip) is freed — so snapshot
   lifetime/layout is not underspecified.
