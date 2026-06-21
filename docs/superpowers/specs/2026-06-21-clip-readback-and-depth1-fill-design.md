# Clip-mask readback + depth-1 fill — design

**Status:** draft, 2026-06-21. Branch
`diag/gkrellm-syncboundary-attribution`. Follows the attribution
investigation (commits `8d15d1ab`, `ce8fc756` on this branch) and two
codex direction reviews (gpt-5.5 2026-06-21, gpt-5.4 2026-06-21).

**One-line framing:** two synchronous `engine.get_image` readbacks on
paint-adjacent hot paths drive the entire gkrellm/cinnamon SyncBoundary
storm. Kill both with targeted, independent fixes — no GPU-side stencil
clip, no CPU-shadow, no Phase B.4.

## Measured root cause (not theory)

Per-call-site `get_image` attribution on bee (AMD RADV APU, gkrellm
under cinnamon, `YSERVER_LOOP_TELEMETRY=1`):

- `submit_group_flush_reason_sync_boundary/s = 2 × get_image_calls/s`,
  exactly (`promote_exportable_runs=0`). The storm **is**
  `engine.get_image` — each call closes the open frame, submits, and
  waits (2 SyncBoundary flushes, engine.rs:4127).
- `get_image` splits ~50/50 between exactly two sites; all others ~0:
  - `clip` (`read_clip_mask_bytes`) ~200/s
  - `cpufill` (`fill_solid_rects_cpu_fallback`) ~250/s
- `cpufill_reason`: **100% `depth_lt8`**, `partial_planemask=0`, and
  **100% `d1_gxcopy`**, `d1_noncopy=0`. Every CPU-fallback fill is a
  depth-1 GXcopy fill.
- `clip_cache` use-path: **100% hit**, `miss_other_xid=0`,
  `miss_no_entry=0`. The clip mask is a single static pixmap; the
  readback is **not** on the paint/use path.

The clip readbacks therefore come from the clip *install* path
(`apply_clip_state`, backend.rs:12356), whose `_ => clip_mask_cache =
None` branch discards the cached bytes every time the client toggles
`clip-mask=None` (for an unclipped op) and re-installs the same pixmap
— the wmaker pattern the code comment already names.

## Design

Three components. **Component 1** (`content_version`) is the shared
correctness primitive **FIX A** depends on; **FIX B1** is independent.

### Component 1 — `Drawable.content_version` (correctness primitive)

`Drawable` (store.rs:572) has `presentation_damage_epoch` but it bumps
only when `scene_participating` is true (store.rs:976/995) — an
offscreen clip-mask pixmap's writes bump nothing. FIX A's cache
retention is only correct if it can detect a genuine mask mutation.

**Design (per codex gpt-5.4):**

- Add one ungated `content_version: u64` to `Drawable`, init 0.
- Add **one** store helper `DrawableStore::mark_contents_modified(id)`
  that bumps it (saturating).
- Call it from **successful drawable-write paths at the engine
  mutation layer** — NOT at request dispatch, NOT speculatively. The
  write set is the **eight engine entry points that emit a dst write**:
  `fill_rect_batch`, `logic_fill`, `copy_area`, `put_image`,
  `image_text`, `composite_glyphs`, `render_composite`,
  `render_traps_or_tris`. Wrapper paths are covered by their delegates,
  NOT counted separately: `render_fill_rectangles` wraps
  `render_composite` (engine.rs:6144), `cow_copy_area` delegates to
  `copy_area` (engine.rs:3416), and backend `copy_plane`
  (backend.rs:12908) decomposes into `fill`/`copy` calls — there is no
  distinct engine `copy_plane` op. Bumping in the eight delegates
  therefore covers the wrappers transitively. **`composite_glyphs`
  (engine.rs:5336) is on the list** — it writes a drawable dst and was
  missed in the first draft; omitting it would leave a glyph-painted
  clip pixmap able to go stale.
- **Closure obligation:** the eight-entry claim is load-bearing for
  correctness (a missed writer = stale clip = wrong clipping). The
  plan MUST re-verify, against the `RecordedOp` enum
  (frame_builder.rs) and the engine write paths, that no dst-mutating
  path exists outside these eight (or their delegates) — including
  window-backing realloc/resize, clear-to-background, COW backing
  writes, and SHM `put_image`. If any is found, it joins the set or
  the version work is not ready.
- **No periodic forced re-read.** Codex: "that just hides bugs." The
  belt-and-suspenders is the test matrix below, not a runtime hedge.

**Cache key + the freed-pixmap contract.** XIDs are reusable (store.rs
hands out monotonic `DrawableId`s but XIDs recycle on free), so the
cache records BOTH the `pixmap_xid` (the identity the installed GC clip
names) AND the `DrawableId` + `content_version` captured at read time.
The XID is **not** dropped — it is how a still-installed GC clip is
matched after its source pixmap is freed.

**X11 freed-pixmap semantics are load-bearing here.** `XSetClipMask`
snapshots the bitmap into the GC; freeing the source pixmap afterward
MUST NOT change the GC's clipping (yserver-core resources.rs enforces
this; backend.rs:18820 tests it; the live-path comment at
backend.rs:6006 states it). So the cache is a **frozen snapshot** whose
validity does not depend on the source drawable still existing. The
version check only applies *while a live drawable still exists and
could be re-read*.

**Readiness gate (codex):** if the writer set cannot be enumerated with
confidence, the version work is not ready. The enumeration above is the
B.3 op list; the test matrix verifies each member bumps.

### FIX A — retain clip-mask cache across `clip → None`

`ClipMaskCache` (kms/backend.rs) gains `drawable_id: DrawableId` and
`content_version: u64` alongside the existing bytes/origin/dims.

1. **`apply_clip_state` (backend.rs:12356)** — the `_ =>` (None / non-
   pixmap) branch clears the *active clip state* (`core.current_clip`)
   but **no longer drops `clip_mask_cache`**. The cached bytes survive a
   `clip → None` toggle.
2. **Reuse condition** — applied at **all three clip-install/use sites**:
   `apply_clip_state` (backend.rs:12356), `intersect_with_current_clip_live`
   (backend.rs:6010), **and `set_clip_pixmap` (backend.rs:12303)**, which
   today eagerly re-reads on every install and must adopt the same
   frozen-snapshot reuse policy (codex round-2 should-fix — otherwise that
   install path keeps re-reading). For an installed
   `ClipState::Pixmap { xid, origin }` whose cache entry has matching
   `pixmap_xid`:
   - **Source drawable freed** (`lookup(xid)` is `None`): **reuse the
     frozen snapshot.** This is the X11 retain-after-free contract; the
     bytes cannot and need not be re-read.
   - **Source drawable live** (`lookup(xid) == Some(did)`): reuse iff
     `did == cache.drawable_id` **and** `drawable.content_version ==
     cache.content_version`; else `read_clip_mask_bytes` and refresh
     the entry (capturing the current `did` + `content_version`). The
     `did` check catches XID-realloc-to-a-different-pixmap (lookup
     returns a new `DrawableId`); the version check catches an
     in-place mask rewrite.
   - On any reuse, update `origin` only.
3. **`free_pixmap`** — **does NOT evict on a plain free** (that would
   break retain-after-free and the backend.rs:18820 test). A freed XID
   later re-allocated to a different pixmap is handled at reuse time by
   the `did` mismatch above, so no eviction is needed for correctness.
   (Confirm the existing free_pixmap cache handling at backend.rs:11754
   matches this — adjust only if it currently evicts unconditionally.)

Single-entry cache is retained for now (telemetry shows
`miss_other_xid=0` — no rotation in the observed workload). A
multi-entry LRU is a trivial later extension if a rotating-mask client
appears; deferred (YAGNI).

4. **Stale comment cleanup** (codex round-2 nit): the comment at
   backend.rs:6080 describes depth-1 clip bytes as MSB-first, but
   `pack_from_storage` (engine.rs:9270) packs them LSB-first. Fix the
   comment while in this code so it doesn't mislead the cache work.

**Effect:** the static-mask + `None`-toggle pattern becomes a cache hit
on re-install → clip readbacks ~200/s → ~0. A client that genuinely
rewrites its mask bumps `content_version` → correct re-read.

### FIX B1 — GPU depth-1 GXcopy solid fill

`fill_solid_rects` (backend.rs:6376) routes `depth < 8 ||
plane_mask != full_mask` to the CPU readback fallback
(`fill_solid_rects_cpu_fallback`) *before* the GPU paths. The GPU paths
already support R8_UNORM depth-1 dst — `logic_fill` decodes a depth-1
`fg` via `decode_x11_pixel_for_storage(fg, depth, format)`
(engine.rs:3127), and `fill_rect_batch` takes a format-aware
`color: [f32; 4]`. The gate is simply over-conservative.

**Design:** add a depth-1 GXcopy fast path to the dispatch — when
`depth == 1` **and** `plane_mask == full_mask` **and**
`function == GXcopy`, route to the GPU fill (`fill_rect_batch` with the
depth-1-decoded color, or `logic_fill` with `Copy`) instead of the CPU
fallback.

**Correctness rationale (corrected):** `decode_x11_pixel_for_storage`
(engine.rs:9337) maps a depth-1 `fg` to `(fg & 0xff) / 255.0`, so
`fg=1` becomes the R8 byte `0x01` — **not** `0xFF`. That is still
correct for GXcopy because `pack_from_storage` (engine.rs:9270) reads
depth-1 back as `raw != 0` (any nonzero byte packs as the set bit, LSB
ordering per kms/backend.rs:40). A GXcopy fill only *writes* (no
read-modify), so there is no R8-vs-boolean hazard. The non-Copy hazard
(`XOR(0xFF,0x01)=0xFE` still nonzero ⇒ wrongly "set") is precisely why
B2 is excluded.

**Scope is `depth == 1` only, not `depth < 8`.** Depth-4 also uses
`R8_UNORM` storage (platform.rs:1524) but the
nonzero-packs-as-set argument is depth-1-specific; depth-4 has no
equivalence proof or test here, so depth-4 (and any other `depth < 8`)
stays on the CPU fallback. Telemetry cannot distinguish depth-1 from
depth-4 within `depth_lt8`, but real clip/stipple masks are depth-1;
if a depth-4 fill rate ever shows up it gets its own proof + tests.

**Explicitly still on the CPU fallback** (all cold per telemetry, so no
perf loss): depth-1 **non-Copy** logic fills (`d1_noncopy=0` — would
need true 1-bit boolean semantics, FIX B2, deferred), **partial
plane-mask** fills (`partial_planemask=0`), and **depth-4** (and any
other `depth < 8` ≠ 1).

The decision between extending `fill_rect_batch` vs reusing
`logic_fill(Copy)` is left to the plan; both already handle R8 depth-1
dst, so it is a code-shape choice, not a capability gap.

## Correctness analysis

- **Static mask + `None` toggle** (observed): re-install hits the
  retained cache (same `DrawableId`, unchanged `content_version`). No
  readback.
- **Client rewrites the mask between uses:** the rewrite is a paint op
  on the mask drawable → `mark_contents_modified` bumps
  `content_version` → reuse condition fails → re-read. Correct clip.
- **Freed source pixmap, GC clip still installed:** `lookup(xid)` is
  `None` → reuse the frozen snapshot (X11 retain-after-free). No
  re-read of a freed drawable.
- **XID reuse after free:** a new pixmap at the same XID resolves to a
  different `DrawableId` → `did` mismatch at reuse → re-read. No stale
  hit. (No eviction on free required.)
- **depth-1 GXcopy on GPU:** write-only; `decode_x11_pixel_for_storage`
  writes `fg & 0xff` (e.g. `0x01` for `fg=1`) and `pack_from_storage`
  packs any nonzero R8 byte as the set bit (LSB order). Matches the CPU
  path's result. No boolean-logic hazard (that's why non-Copy is
  excluded).
- **`content_version` overflow:** `u64` saturating; wrap is
  astronomically out of reach and a saturating bump degrades to "always
  re-read," never "false hit."

## Tests

Unit (kms::v2 lib), codex-mandated matrix:

- One per write primitive — the load-bearing enumeration check, **all
  eight**: `content_version_bumps_on_{fill_rect_batch, logic_fill,
  copy_area, put_image, image_text, composite_glyphs, render_composite,
  render_traps_or_tris}`. (composite_glyphs + render_traps_or_tris were
  missing from the first draft — they are the easy-to-forget writers.)
- `clip_cache_retained_across_clip_none_same_pixmap_no_reread` — set
  mask, paint, set None, re-set same mask: assert zero additional
  `read_clip_mask_bytes`.
- `clip_cache_retained_after_source_pixmap_freed` — set mask, free the
  source pixmap, paint clipped: assert the cached snapshot is reused
  (no re-read, no error) and clipping still matches the mask. Mirrors
  the existing backend.rs:18820 retain-after-free expectation.
- `clip_cache_invalidated_on_mask_put_image` — write the live mask
  pixmap between uses: assert a re-read (version bump).
- `clip_cache_invalidated_on_mask_copy_area` — same via CopyArea.
- `clip_cache_invalidated_on_mask_depth1_gpu_fill` — write the mask via
  the FIX B1 GPU depth-1 fill, then use it as a clip: assert the fill
  bumped `content_version` and the next clip install/use re-reads
  (end-to-end A↔B1 interaction).
- `clip_cache_free_realloc_same_xid_no_stale_hit` — free, re-allocate a
  different pixmap at the same XID, install as clip: assert `did`
  mismatch forces a re-read (no stale frozen-snapshot hit).
- `depth1_gxcopy_fill_routes_to_gpu_not_cpu_readback` — assert the
  depth-1 GXcopy fill takes the GPU path (no `get_image` call).
- `depth1_noncopy_fill_still_cpu_fallback` — guard B2's exclusion.
- `depth4_fill_still_cpu_fallback` — guard the `depth == 1`-only scope.

Integration (`tests/v2_acceptance.rs`, Vulkan-gated, `--ignored` under
lavapipe): the existing clip-correctness suite must stay green
(`v2_clip_pixmap_mask_gates_poly_fill_to_mask_shape`,
`composite_glyphs_clip`, `copy_plane_depth1`, `read_depth1`,
`render_composite_no_gc_clip_leak`). Add
`v2_depth1_gxcopy_fill_matches_cpu_reference` — fill a depth-1 pixmap on
the GPU path, read it back, assert identical to the CPU-fallback result.

## Hardware gate (user-run, bee)

Same gkrellm/cinnamon workload, telemetry on:

- `get_image_by_site[clip]/s` → ~0 (FIX A), `[cpufill]/s` → ~0 (FIX B1).
- `get_image_calls/s` → ~0, `submit_group_flush_reason_sync_boundary/s`
  → low.
- Felt result: the pegged core drops, choppiness gone.
- No regression: bee MATE drag stays smooth (the B.3 win);
  `frame_builder_aborts/s` = 0; no `ERROR_DEVICE_LOST`. Clip rendering
  visually correct (wmaker title-bar buttons, gkrellm graphs).

## Non-goals (explicitly deferred)

- **FIX B2 — depth-1 non-Copy logic fills** with true 1-bit boolean
  semantics. Cold (`d1_noncopy=0`); revisit only if telemetry shows
  them. Byte-wise R8 `logic_fill` is wrong for these.
- **Partial-plane-mask fills.** Cold (`partial_planemask=0`).
- **GPU-side stencil/shader clip application.** A larger structural
  end-state; unnecessary now that FIX A retains the CPU mask cheaply.
- **CPU shadow for all depth-1 pixmaps.** Codex: not cheaper than A+B,
  same coherence problem at larger surface.
- **Multi-entry clip cache (LRU).** No rotation observed
  (`miss_other_xid=0`); trivial later extension.
- **Phase B.4 (compose-into-frame) / B.5.** The bottleneck is
  readback-triggered sync, not frame-builder completion; cap stays at 1
  (RDNA2/RADV multi-CB safety rail).

## Implementation order (codex-endorsed)

1. Add `Drawable.content_version` + `mark_contents_modified`, wire the
   eight writer bumps.
2. Extend `ClipMaskCache` with `drawable_id` + `content_version`;
   implement the reuse rules in `apply_clip_state`,
   `intersect_with_current_clip_live`, `set_clip_pixmap`, and
   `free_pixmap` (no-evict-on-free); fix the stale bit-order comment.
3. Add the depth-1 GXcopy GPU branch in `fill_solid_rects`.
4. Run the writer-matrix tests, the free/realloc clip-cache tests, and
   the depth-1 fill routing/reference tests; then bee HW gate.

## Review trail

- Direction reviews (codex): gpt-5.5 + gpt-5.4 2026-06-21 — confirmed
  attack the readback, not Phase B.4; corrected DrawableId-vs-XID keying,
  content_version discipline, depth-1 boolean semantics (B1 GXcopy-only).
- Design reviews (codex gpt-5.4): round 1 NO-GO (freed-pixmap semantics,
  writer-set closure); round 2 **GO** after the revisions in this doc,
  with the `set_clip_pixmap` should-fix and bit-order nit folded in above.

## References

- Attribution + disambiguation telemetry: commits `8d15d1ab`,
  `ce8fc756` (this branch).
- Findings + codex reviews: `project_client_scheduling_fairness`
  (auto-memory).
- Clip install: `backend.rs:12356` (`apply_clip_state`); clip use:
  `backend.rs:6010` (`intersect_with_current_clip_live`); mask read:
  `backend.rs:6084` (`read_clip_mask_bytes`).
- Fill dispatch: `backend.rs:6376` (`fill_solid_rects`); CPU fallback:
  `backend.rs:6502`; GPU paths: `engine.rs:2921` (`fill_rect_batch`),
  `engine.rs:3074` (`logic_fill`); depth-1 pixel decode:
  `engine.rs:3127` (`decode_x11_pixel_for_storage`).
- `get_image` double-SyncBoundary: `engine.rs:4127`.
- Drawable epoch (gated — why it can't be reused): `store.rs:594`.
- `DrawableId` allocation (monotonic, why cache keys on it):
  `store.rs:770`.
