# Clip-mask readback + depth-1 fill — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Eliminate the two `engine.get_image` readbacks that drive the gkrellm/cinnamon SyncBoundary storm — the per-clip-install clip-mask readback and the depth-1 GXcopy solid-fill CPU read-modify-write.

**Architecture:** Add an ungated per-`Drawable` `content_version` write counter; make the clip-mask cache a frozen snapshot that survives `clip→None` toggles and source-pixmap free, re-reading only when a live mask's `content_version` actually changes; route depth-1 GXcopy fills to the existing GPU fill path instead of the CPU `get_image` RMW.

**Tech Stack:** Rust, Vulkan (ash), the yserver v2 KMS backend. Spec: `docs/superpowers/specs/2026-06-21-clip-readback-and-depth1-fill-design.md`.

**Verification commands (run from repo root):**
- Unit: `cargo test -p yserver --locked --lib <name>`
- Lavapipe integration: `cargo test -p yserver --test v2_acceptance -- --ignored <name>`
- Pre-commit: `cargo fmt`, `cargo clippy -p yserver --locked`, `cargo test -p yserver --locked --lib`

---

## File Structure

- `crates/yserver/src/kms/v2/store.rs` — `Drawable.content_version` field + `DrawableStore::mark_contents_modified`.
- `crates/yserver/src/kms/v2/engine.rs` — bump `content_version` in the eight write entry points.
- `crates/yserver/src/kms/backend.rs` — `ClipMaskCache` gains `drawable_id` + `content_version`; fix stale bit-order comment.
- `crates/yserver/src/kms/v2/backend.rs` — clip-cache reuse rules in `apply_clip_state`, `intersect_with_current_clip_live`, `set_clip_pixmap`, `free_pixmap`; depth-1 GXcopy dispatch in `fill_solid_rects`.
- `crates/yserver/tests/v2_acceptance.rs` — lavapipe writer-bump + depth-1-fill equivalence tests.

---

## Task 1: `content_version` field + `mark_contents_modified` helper

**Files:**
- Modify: `crates/yserver/src/kms/v2/store.rs` (`Drawable` struct ~572, init ~793, new method on `DrawableStore`)
- Test: `crates/yserver/src/kms/v2/store.rs` (tests module)

- [ ] **Step 1: Add the field.** In `struct Drawable` (store.rs:572), after `presentation_damage_epoch: u64`, add:

```rust
/// Ungated monotonic content-write counter. Bumped on EVERY write to
/// this drawable's pixels (the eight engine paint entry points),
/// regardless of `scene_participating` — unlike `presentation_damage_epoch`
/// which is gated and so misses offscreen clip-mask writes. Consumed by
/// the clip-mask cache to detect a genuine mask mutation vs a cheap
/// clip-install re-toggle. Saturating; wrap degrades to "always re-read".
pub(crate) content_version: u64,
```

- [ ] **Step 2: Initialise it.** At the `Drawable { … }` construction with `presentation_damage_epoch: 0` (store.rs:793), add `content_version: 0,`.

- [ ] **Step 3: Write the failing test** in the store.rs tests module, using the **real** store-test APIs (verified: `DrawableId::for_tests` at store.rs:56, `DrawableStore::get_mut` at 839, allocation via `DrawableStore::allocate(...)` at 770 whose real signature is `(xid, DrawableKind, depth: u8, scene_participating: bool, Storage) -> Result<DrawableId, AllocError>` — match the neighbouring tests at store.rs:1195, e.g. `.allocate(.., 32, false, stub_storage()).unwrap()`):

```rust
#[test]
fn mark_contents_modified_bumps_content_version() {
    let mut s = DrawableStore::new();
    let id = s.allocate(/* xid */, DrawableKind::Pixmap, 32, false, stub_storage()).unwrap();
    assert_eq!(s.get(id).unwrap().content_version, 0);
    s.mark_contents_modified(id);
    s.mark_contents_modified(id);
    assert_eq!(s.get(id).unwrap().content_version, 2);
    // Unknown id is a silent no-op (never panics).
    s.mark_contents_modified(DrawableId::for_tests(u64::MAX));
}
```

- [ ] **Step 4: Run it, expect FAIL** (`mark_contents_modified` undefined):
Run: `cargo test -p yserver --locked --lib mark_contents_modified_bumps_content_version`

- [ ] **Step 5: Implement the helper** on `impl DrawableStore`:

```rust
/// Bump a drawable's `content_version` (saturating). Call on every
/// successful pixel write. No-op for an unknown id.
pub(crate) fn mark_contents_modified(&mut self, id: DrawableId) {
    if let Some(d) = self.get_mut(id) {
        d.content_version = d.content_version.saturating_add(1);
    }
}
```

(Use the store's existing mutable-accessor; if it isn't named `get_mut`, use the real one.)

- [ ] **Step 6: Run it, expect PASS.**

- [ ] **Step 7: Commit.**

```bash
git add crates/yserver/src/kms/v2/store.rs
git commit -m "feat(v2/store): add ungated Drawable.content_version + mark_contents_modified"
```

---

## Task 2: Wire `content_version` bumps into the eight engine writers

**Files:**
- Modify: `crates/yserver/src/kms/v2/engine.rs` — the eight write entry points.
- Test: `crates/yserver/tests/v2_acceptance.rs` (lavapipe-gated — these ops need Vk).

The eight entry points and the destination drawable each writes:
- `fill_rect_batch` (2921) — `target`
- `logic_fill` (3074) — `target`
- `copy_area` (3223) — the dst id
- `put_image` (3987) — `id`
- `image_text` (4404) — the dst id
- `composite_glyphs` (4782) — the dst id
- `render_composite` (5400) — the dst id
- `render_traps_or_tris` (6216) — the dst id

Wrappers are covered transitively and must NOT get their own bump: `fill_rect`→`fill_rect_batch` (2888), `cow_copy_area`→`copy_area` (3416), `render_fill_rectangles`→`render_composite` (6144).

- [ ] **Step 1: Closure-obligation audit (do this first, write findings into the commit message).** Confirm no dst-mutating path exists outside these eight **or their delegates**. The proof is "the eight engine entry points + the known backend delegates that funnel into them" — NOT `RecordedOp` alone. Codex verified: window-backing resize is a fresh-`DrawableId` replacement (store.rs:858/927), not an in-place write (no bump needed); SHM/core `put_image` reaches `engine.put_image` (backend.rs:13095). Backend write delegates that collapse into the eight: `fill_rect`→`fill_rect_batch` (engine.rs:2888), `cow_copy_area`→`copy_area` (3416), `render_fill_rectangles`→`render_composite` (6144), backend `copy_plane`→fill/copy (backend.rs:12908), and **`create_pixmap`'s initial clear → `engine.fill_rect`** (backend.rs:11749) and any other background/initialization fill. Confirm each such delegate reaches one of the eight (so it inherits the bump); if any path writes pixels WITHOUT routing through the eight, it joins the bump set before proceeding.

- [ ] **Step 2: Add the bump — placement is per-commit-point, NOT adjacent to first_touch.** Several writers first-touch the dst at the top but still have early returns *before* the op is committed (`image_text` 4404 returns on empty `glyphs_to_draw`; `composite_glyphs` 4782 has post-prelude `Ok(stats)` exits before the `RecordedOp::CompositeGlyphs` push; `render_composite` 5400 resolves src/mask/view/descriptors and has later no-op branches). **Uniform safe rule: bump immediately AFTER the op is irrevocably committed to the open frame — i.e. right after its `RecordedOp::…` push (or, for `render_composite`, after its render-batch append).** This is past every early-return, so no over-bump. Codex verified the exact post-append commit point in each (insert the bump right after these lines): `fill_rect_batch` engine.rs:3009, `logic_fill` 3200, `copy_area` 3390, `put_image` 4119, `image_text` 4730, `composite_glyphs` 5331, `render_composite` 6102, `render_traps_or_tris` 6671/6679. Use the post-append point everywhere for consistency:

```rust
// immediately after the RecordedOp::<Op> push / render-batch append:
store.mark_contents_modified(<dst id for this op>);
```

(Record-time bump = the client's write intent, matching damage semantics; the spec's A↔B1 ordering analysis — confirmed by codex against engine.rs:4155 — shows `get_image` later closes+flushes the frame, so the GPU write is visible before any clip re-read.)

- [ ] **Step 3: Add an xid-based test accessor.** The integration crate cannot name `DrawableId` (that's why `allocate_test_pixmap_bgra` returns `Option<u32>`, backend.rs:3277). Add to the v2 backend:

```rust
pub fn drawable_content_version_for_tests(&self, host_xid: u32) -> Option<u64> {
    let id = self.store.lookup(host_xid)?;
    self.store.get(id).map(|d| d.content_version)
}
```

- [ ] **Step 4: Write the failing writer-bump tests.** Cover the **representative subset that has existing lavapipe helpers** (`tests/v2_acceptance.rs`, model on the `#[ignore]` tests + the xid-based engine helpers at backend.rs:3814/3884/3917/3945/3984/4074): `fill_rect_batch`, `copy_area`, `put_image`, `render_composite`, `render_traps_or_tris`.

```rust
#[test]
#[ignore = "needs Vulkan ICD (lavapipe)"]
fn content_version_bumps_on_fill_rect_batch() {
    let mut be = /* v2_acceptance Vk fixture */;
    let xid = be.allocate_test_pixmap_bgra(/* … */).unwrap();
    let v0 = be.drawable_content_version_for_tests(xid).unwrap();
    /* drive the fill via the existing xid-based engine test helper */;
    assert!(be.drawable_content_version_for_tests(xid).unwrap() > v0);
}
```

For `image_text` and `composite_glyphs` there is **no lavapipe helper today** (the `composite_glyphs_for_tests` TODO at v2_acceptance.rs:3895). Do NOT silently skip them: the uniform post-append placement (Step 2) makes their bump correct-by-construction, and the closure audit (Step 1) covers them — state this explicitly in the commit message as the coverage rationale for those two, rather than adding two heavyweight helpers. `logic_fill`'s bump is exercised transitively by the depth-1 fill test in Task 4 (which routes through `logic_fill(Copy)`).

- [ ] **Step 5: Run them, expect FAIL** (no bump yet / accessor missing):
Run: `cargo test -p yserver --test v2_acceptance -- --ignored content_version_bumps_on_`

- [ ] **Step 6: Implement the bumps (Step 2) + accessor (Step 3); run, expect PASS.**

- [ ] **Step 7: Commit.**

```bash
git add crates/yserver/src/kms/v2/engine.rs crates/yserver/tests/v2_acceptance.rs crates/yserver/src/kms/v2/backend.rs
git commit -m "feat(v2/engine): bump content_version on the eight drawable-write entry points"
```

---

## Task 3: Clip-mask cache retention (frozen snapshot, version-guarded)

**Files:**
- Modify: `crates/yserver/src/kms/backend.rs` — `ClipMaskCache` struct (48) + stale comment (~6080 region of v2/backend.rs is the comment; the bit-order comment in this file's rasteriser header).
- Modify: `crates/yserver/src/kms/v2/backend.rs` — `apply_clip_state` (12356), `intersect_with_current_clip_live` (6010), `set_clip_pixmap` (12303), `free_pixmap` (11754).
- Test: `crates/yserver/src/kms/v2/backend.rs` tests (seed-the-cache, no Vk — model on `apply_clip_state_preserves_cached_pixmap_mask_after_free_and_origin_change` at 18820).

- [ ] **Step 1: Extend `ClipMaskCache`** (kms/backend.rs:48) — add after `pixmap_xid`:

```rust
/// Live drawable identity captured at read time. `pixmap_xid` is the
/// installed-GC handle (survives free); `drawable_id` distinguishes a
/// re-allocated pixmap at a recycled xid.
pub(crate) drawable_id: crate::kms::v2::store::DrawableId,
/// `Drawable.content_version` at the moment the bytes were read. While a
/// live drawable still exists, reuse requires this to still match.
pub(crate) content_version: u64,
```

`DrawableId` is already `pub(crate)` (store.rs:56), so store it directly — no raw-`u64` fallback needed.

- [ ] **Step 2: Define the shared reuse predicate.** Add a private helper on the v2 backend:

```rust
/// True iff the cached clip mask may be reused for installed pixmap `xid`
/// without a fresh readback. Frozen-snapshot policy:
/// - source freed (`lookup(xid) == None`)  -> reuse (X11 retain-after-free)
/// - source live                            -> reuse iff same DrawableId
///   AND unchanged content_version.
fn clip_cache_reusable(&self, xid: u32) -> bool {
    let Some(cache) = self.clip_mask_cache.as_ref() else { return false; };
    if cache.pixmap_xid != xid { return false; }
    match self.store.lookup(xid) {
        None => true, // freed: frozen snapshot is the GC's retained copy
        Some(did) => did == cache.drawable_id
            && self.store.get(did).is_some_and(|d| d.content_version == cache.content_version),
    }
}
```

- [ ] **Step 3: Make `read_clip_mask_bytes` capture identity + version.** Where it builds the `ClipMaskCache` (kms/v2/backend.rs:6072+), set `drawable_id` to the resolved id and `content_version` to that drawable's current `content_version`.

- [ ] **Step 4: Rewrite the install/use sites** to use the predicate. Real shapes (verified):
  - `apply_clip_state` (12356) and `intersect_with_current_clip_live` (6010) match `ClipState::Pixmap { origin, pixmap }` — get the xid via `pixmap.as_raw()`.
  - `set_clip_pixmap` (12297) takes a raw `host_pixmap: u32` directly (not a `ClipState`).

  At each: if `self.clip_cache_reusable(xid)` → update only `cache.origin = origin` (where an origin is available); else → `self.clip_mask_cache = self.read_clip_mask_bytes(xid, origin)`. In `apply_clip_state`, the `_ =>` branch sets `core.current_clip` to the new state but **must NOT** touch `clip_mask_cache`.

- [ ] **Step 5: Fix `free_pixmap` (11754)** — remove any unconditional `clip_mask_cache = None` on plain free. (XID-realloc is handled by the `drawable_id` mismatch in the predicate, so no eviction is needed for correctness. If existing teardown requires clearing on some path, scope it narrowly and document why.)

- [ ] **Step 6: Fix the stale comment** — update the depth-1 bit-order comment (the `read_clip_mask_bytes` doc-comment / kms/backend.rs rasteriser header) from MSB-first to LSB-first, matching `pack_from_storage` (engine.rs:9270).

- [ ] **Step 7: Write the failing tests as crate-local backend `#[cfg(test)]` unit tests** (no Vk — seed `clip_mask_cache` directly, model on `apply_clip_state_preserves_cached_pixmap_mask_after_free_and_origin_change` at backend.rs:18820). These MUST be crate-local (not `v2_acceptance`) because they assert on `self.telemetry` counters, which have no public acceptance getter — the "no re-read" assertion reads `telemetry.bucket.clip_mask_reads` / `get_image_by_site[ClipMask]` (`read_clip_mask_bytes` records `GetImageSite::ClipMask` at backend.rs:6112) directly:

```rust
// clip_cache_retained_across_clip_none_same_pixmap_no_reread:
//   seed cache for (xid, did, version); apply_clip_state(None);
//   snapshot clip_mask_reads; apply_clip_state(Pixmap{same pixmap});
//   assert cache still present + same bytes + clip_mask_reads UNCHANGED.
// clip_cache_retained_after_source_pixmap_freed:
//   seed cache; free the source drawable; assert clip_cache_reusable(xid) == true.
// clip_cache_invalidated_on_content_version_bump:
//   seed cache at version V; mark_contents_modified(mask id); assert reusable == false.
// clip_cache_free_realloc_same_xid_no_stale_hit:
//   seed cache for (xid, did=A); free + re-allocate a new pixmap at the same xid
//   (did=B); assert reusable == false.
```

- [ ] **Step 8: Run, expect FAIL → implement (Steps 2-6) → PASS.**
Run: `cargo test -p yserver --locked --lib clip_cache_`

- [ ] **Step 9: Commit.**

```bash
git add crates/yserver/src/kms/backend.rs crates/yserver/src/kms/v2/backend.rs
git commit -m "feat(v2/clip): retain pixmap clip cache across None using drawable content_version"
```

---

## Task 4: depth-1 GXcopy GPU fill

**Files:**
- Modify: `crates/yserver/src/kms/v2/backend.rs` — `fill_solid_rects` dispatch (6376).
- Test: `crates/yserver/tests/v2_acceptance.rs` (lavapipe) + a unit guard.

- [ ] **Step 1: Add the depth-1 GXcopy fast path** in `fill_solid_rects`, intercepting **before** the `depth < 8 || plane_mask != full_mask` gate at backend.rs:6382. Critically: at that gate, `color`/`vk_rects` are NOT yet built (`vk_rects` is created later at 6465), but `shifted` (the offset-adjusted rects) **is** already in scope (built at ~6399). So call `engine.logic_fill` with `GcFunction::Copy` — it already does the depth-1 R8 decode (`decode_x11_pixel_for_storage`) and takes `shifted`-shaped rects, mirroring the existing non-Copy branch at 6419:

```rust
use yserver_core::backend::GcFunction;
// depth-1 + full plane mask + GXcopy: write-only, so the GPU R8 fill is
// correct (decode writes fg&0xff; pack_from_storage packs nonzero as the
// set bit, LSB). depth-1 non-Copy (boolean-logic hazard) and depth-4
// (no equivalence proof) stay on the CPU fallback.
if depth == 1 && plane_mask == full_mask && matches!(function, GcFunction::Copy) {
    let opaque_alpha = depth != 32; // = true here; matches the 6418 policy
    match self.engine.logic_fill(
        &mut self.store, &mut self.platform, id,
        GcFunction::Copy, opaque_alpha, fg & full_mask, &shifted,
    ) {
        Ok(()) => { self.telemetry.record_paint_submit(); /* trace as FillBatch */ return; }
        Err(e) => { log::warn!("v2 fill_solid_rects depth1 gpu copy: {e:?}"); return; }
    }
}
```

Keep the `depth < 8 || plane_mask != full_mask` CPU-fallback gate intact for everything this fast path doesn't claim (depth-1 non-Copy, depth-4, partial plane mask).

- [ ] **Step 2: Write the failing tests.** Counter-based assertions (`get_image_by_site[CpuFallbackFill]`) are NOT observable from `tests/v2_acceptance.rs` (the integration crate can't read `self.telemetry`), so they go in **crate-local backend `#[cfg(test)]` tests using `for_tests_with_vk()`** (which builds a real Vk backend in-crate and exposes `self.telemetry`); only byte-readback equivalence (which uses the public `get_image_pixels_for_tests`, backend.rs:681) goes in lavapipe `v2_acceptance`.
  - **Crate-local (for_tests_with_vk + telemetry):**
    - `depth1_gxcopy_fill_routes_to_gpu_not_cpu_readback` — depth-1 GXcopy fill: assert `telemetry…get_image_by_site[CpuFallbackFill]` does NOT increment.
    - `depth1_noncopy_fill_still_cpu_fallback` and `depth4_fill_still_cpu_fallback` — assert the CpuFallbackFill counter DOES increment (B1 scope boundary holds).
    - `clip_cache_invalidated_on_mask_depth1_gpu_fill` (A↔B1 e2e, design-required, codex #9) — create a depth-1 pixmap, install as clip mask (populates cache), write it via the new depth-1 GXcopy GPU fill, use as clip again: assert the fill bumped `content_version` and the next clip use **re-reads** (`clip_mask_reads` increments) reflecting the new content.
  - **Lavapipe (`v2_acceptance.rs`):**
    - `v2_depth1_gxcopy_fill_matches_cpu_reference` — fill a depth-1 pixmap via the new GPU path, read back via `get_image_pixels_for_tests`, assert byte-identical to the CPU-fallback result for the same rects/fg.

- [ ] **Step 3: Run, expect FAIL → implement Step 1 → PASS.**
Run: `cargo test -p yserver --test v2_acceptance -- --ignored depth1`

- [ ] **Step 4: Commit.**

```bash
git add crates/yserver/src/kms/v2/backend.rs crates/yserver/tests/v2_acceptance.rs
git commit -m "feat(v2/fill): GPU-path depth-1 GXcopy solid fills instead of get_image RMW"
```

---

## Task 5: Regression + correctness gate

**Files:** none (verification only).

- [ ] **Step 1: Existing clip-correctness suite stays green** (lavapipe):
Run: `cargo test -p yserver --test v2_acceptance -- --ignored`
Expected: `v2_clip_pixmap_mask_gates_poly_fill_to_mask_shape`, `composite_glyphs_clip`, `copy_plane_depth1`, `read_depth1`, `render_composite_no_gc_clip_leak` all PASS.

- [ ] **Step 2: Full lib suite + lints green.**
Run: `cargo test -p yserver --locked --lib && cargo clippy -p yserver --locked && cargo fmt --check`
Expected: 0 failures, 0 warnings.

- [ ] **Step 3: Hand off the bee HW gate to the user** (cannot run from this environment). Same gkrellm/cinnamon workload, `YSERVER_LOOP_TELEMETRY=1`:
  - `get_image_by_site/s[clip]` → ~0 and `[cpufill]` → ~0
  - `get_image_calls/s` → ~0, `submit_group_flush_reason_sync_boundary/s` → low
  - pegged core drops, choppiness gone; bee MATE drag still smooth; `frame_builder_aborts/s` = 0; no `ERROR_DEVICE_LOST`; clip rendering visually correct (wmaker title-bar buttons, gkrellm graphs).

---

## Self-Review notes

- **Spec coverage:** Component 1 → Task 1+2; FIX A → Task 3; FIX B1 → Task 4; tests → Tasks 2/3/4 + Task 5 gate. The codex round-2 `set_clip_pixmap` should-fix → Task 3 Step 4; bit-order nit → Task 3 Step 6; closure obligation → Task 2 Step 1.
- **Type consistency:** `mark_contents_modified(DrawableId)` (Task 1) is the single bump entry used in Task 2 and read by `clip_cache_reusable` (Task 3). `ClipMaskCache.{drawable_id,content_version}` defined in Task 3 Step 1, populated in Step 3, read in Step 2.
- **Vk-gating:** per-writer bumps and depth-1 fill need Vk → lavapipe `--ignored` integration tests; the store helper and clip-cache reuse logic are unit-testable by seeding the cache (model on backend.rs:18820).
