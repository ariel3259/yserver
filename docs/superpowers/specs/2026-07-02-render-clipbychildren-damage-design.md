# RENDER ClipByChildren parity — Composite / CompositeGlyphs / Trapezoids / Triangles

**Date:** 2026-07-02
**Branch:** `feat/render-clipbychildren-damage`
**Survey item:** `[T1]` in `docs/superpowers/findings/2026-06-26-stub-gap-survey.md`.

## Goal

Give four RENDER paint ops full X.Org `ClipByChildren` parity on **both** the paint
and the damage side: Composite, CompositeGlyphs, Trapezoids, and Triangles/TriStrip/
TriFan. X.Org clips RENDER paint — and the damage it generates — to the destination
window's `clipList` = its geometry ∩ the destination picture's clip, MINUS its mapped
children, governed by the **destination picture's** `subWindowMode`
(`xserver/render/mipict.c:112`). yserver currently does neither for these ops: it
paints the full destination (stomping shared-backing child pixels) and damages the
full destination drawable (the over-damage class that drove the mate-panel systray
recomposite self-loop, fixed for `FillRectangles` in `e7a1ba0`).

## Design: compute the clipList once (backend), return it, damage exactly it

Two codex reviews drove this design. The naive plan (reuse the GC-keyed
`clip_fill_rects_by_subwindow_mode` for paint + swap the core damage helper) has two
real correctness holes:

1. **Wrong subwindow-mode source.** `clip_fill_rects_by_subwindow_mode`
   (`backend.rs:7130`) gates on `core.current_subwindow_mode`, which is **GC** state
   left over from unrelated core ops. RENDER is governed by the **destination
   picture's** `subWindowMode`, stored on the picture record
   (`crates/yserver/src/kms/core.rs:1391`, updated by `RenderChangePicture`
   `CPSubwindowMode` at `backend.rs:8850`). (`FillRectangles` shares this latent bug;
   out of scope here.)
2. **Paint/damage child-set mismatch.** Paint is clipped backend-side against
   `windows_v2` (which can skip manually-redirected children); damage is accumulated
   core-side against the resources tree (`mapped_child_clip_rects`, which cannot).
   The two disagree on the child set — visible for a translucent manually-redirected
   child, where a parent-backdrop paint under it must be reported so the compositor
   recomposites.

Both holes close if a **single clipList region is computed once, backend-side, gated
on the destination picture's `subWindowMode`, and returned to the core so it damages
exactly the region that was painted.** Paint and damage then use the same region by
construction (no asymmetry), the mode source is correct, and damage becomes precise
to what actually changed.

### Backend trait change (`crates/yserver-core/src/backend/trait_def.rs`)

The four render methods change their return type from `io::Result<()>` to
`io::Result<Vec<Rectangle16>>` — the surviving painted region in **destination-drawable-local**
coordinates (empty ⇒ nothing painted ⇒ no damage). No parameters change.

- `render_composite`, `render_composite_glyphs`, `render_trapezoids`,
  `render_triangles_op`.

Impls:
- **`crates/yserver/src/kms/v2/backend.rs`** (the real work — computes and returns the
  clipList; see below).
- **`crates/yserver-core/src/backend/recording.rs`** (`RecordingBackend`, test double):
  returns a **test-configurable** region (new field, e.g. `render_return_region:
  Vec<Rectangle16>`; default empty). `RecordingBackend` only receives a host xid, not
  the drawable geometry, so it cannot synthesize an extent — the plumbing test sets
  this field explicitly and asserts the core damages exactly it. (The existing
  `render_composite_emits_damage_on_dst_drawable` test is updated to set it.)
  **Note:** `RecordingBackend` currently relies on the trait-**default**
  `render_triangles_op` (`trait_def.rs:1656`) and does not override it. It must add an
  explicit override returning `render_return_region`, or triangles will silently return
  the empty default and drop damage. (The real `KmsBackendV2` overrides all four —
  `render_triangles_op` at `backend.rs:15988`.)
- **`crates/yserver-core/src/host_x11/trait_impl.rs`** (nested): returns empty
  (ynest is unmaintained; nested damage is out of scope).
- **`crates/yserver/tests/v2_acceptance.rs`** — existing call sites of these methods
  (e.g. `:260`, `:385`, `:803`) bind/ignore the old `()` return and need the
  return-type update (test-only, mechanical).

### Paint side — `crates/yserver/src/kms/v2/backend.rs`

All four ops are already structurally identical through the `dst_clip` step
(`resolve_dst_picture_for_render` → `shift_dst_picture_clip`), and already draw via a
multi-rect scissored loop (`build_render_clip_scissors` `engine.rs:10906` →
`ops/render.rs:327` / `ops/text.rs:189`). A shared private helper does the clipList:

```
fn render_dst_cliplist_local(
    &self,
    dst_host_xid: u32,
    pre_shift_picture_clip: Option<&[Rectangle16]>, // local coords, from resolve_dst_picture_for_render
    dst_local_extent: Rectangle16,                  // the dst window's own w×h
    op_bbox_local: Rectangle16,                     // this op's paint extent (see below)
) -> Vec<Rectangle16>
```

computed entirely in **dst-window-local** coords (so no offset juggling — the child
geometries in `windows_v2` are local too), in this order:

1. **Base = picture clip ∩ dst extent.** If the picture set no clip, base = the full
   dst-local extent.
2. **Child subtraction, gated on the PICTURE's `subWindowMode`.** Read
   `subwindow_mode` from the destination `PictureRecord::Drawable`. If
   `ClipByChildren` (the default), subtract every mapped child that is **not**
   manually-redirected (reusing `clip_fill_rects_by_subwindow_mode`'s child
   enumeration — its `scene_participating` skip — but gated on the picture mode, not
   `core.current_subwindow_mode`). If `IncludeInferiors`, skip the subtraction.
3. **∩ op bounding box.** Intersect with `op_bbox_local` so the region is exactly what
   this op paints, in dst-local coords (computed *before* `dst_target.offset` is folded
   in for paint):
   - **Composite** → the `(dst_x, dst_y, width, height)` destination rect.
   - **Trapezoids / Triangles / TriStrip / TriFan** → the primitive bbox already
     computed for scissoring (`rt.bbox_*`).
   - **CompositeGlyphs** → the **union of the actually-rendered glyph destination
     quads**, not the request envelope. The glyph path already accumulates multiple
     glyph elements (with per-element position deltas) and projects their dst boxes
     (`engine.rs:5731,5779`); the returned region reuses that projected union.

The result is the local clipList. The op then (a) shifts it by `dst_target.offset`
via the existing `shift_dst_picture_clip` and feeds the existing scissor path
(so paint is clipped identically to before, but now also by children), and
(b) **returns the un-shifted local region** as the method's value for the core to
damage. `compute_copy_area_scissors` (`backend.rs:2411`) is the in-tree template for
this local-compute-then-shift pattern.

### Damage side — `crates/yserver-core/src/core_loop/process_request.rs`

At the three arms (Composite `:1628`/`:1671`, Traps/Tris `:1675`/`:1745`,
CompositeGlyphs `:1806`/`:1849`), replace the current
`accumulate_damage_full_to_state(state, dst_drawable)` with: capture the region
returned by the backend call and accumulate damage over **each returned rect**
(`accumulate_damage_to_state(state, dst_drawable, r.x, r.y, r.width, r.height)`). An
empty region ⇒ no damage. The core no longer computes the child set itself for these
ops — it damages exactly what the backend reports it painted.

Op 22 (FreeGlyphs) emits no damage and is untouched.

## Correctness notes

- **F1 fixed:** child subtraction is gated on the destination picture's
  `subWindowMode`, read from the picture record — the X.Org-correct source. A picture
  set to `IncludeInferiors` is not child-clipped, on either paint or damage.
- **F2 fixed by construction:** one region drives both paint scissors and damage, so
  they cannot disagree. Because paint does not subtract manually-redirected children
  (they own their backing and composite on top), the returned region *includes* the
  area under them — so a parent-backdrop paint under a translucent manually-redirected
  child *is* reported, and the compositor recomposites. No opaque-overlay assumption
  needed.
- **Precise damage:** the returned region is clipList ∩ op-bbox, so damage matches the
  pixels actually painted — tighter than the old whole-drawable damage and tighter
  than `FillRectangles`' whole-drawable-minus-children.
- **Pixmap destinations:** no `windows_v2` entry ⇒ no children subtracted; region =
  picture clip ∩ pixmap extent ∩ op bbox. Unaffected by children.

## Testing (TDD — red first)

Backend clip-logic tests (`crates/yserver/src/kms/v2/backend.rs`, mirroring
`clip_fill_rects_by_subwindow_mode_*` and `copy_area_clip_by_children_*`):

1. Per op: dst window + mapped automatic child overlapping the paint ⇒ returned
   region and scissor rects = (op-bbox ∩ window) − child. Red vs current full-dst
   paint/return.
2. **IncludeInferiors regression:** destination picture with `subWindowMode =
   IncludeInferiors` ⇒ children are **not** subtracted (guards F1). This is the test
   the previous spec was missing.
3. **Manually-redirected child:** not subtracted (region covers under it) — reuses the
   `copy_area_clip_by_children_skips_manually_redirected_child` fixture shape;
   guards F2.
4. **Source/mask clip fold:** child subtraction composed with a non-empty src/mask
   picture clip (`compute_render_composite_clip`) still yields the correct region
   (guards against breaking the existing fold).
5. **Coordinate/offset:** redirected dst (non-zero `dst_target.offset`) ⇒ scissors are
   correctly shifted while the returned region stays in local coords.
6. Fully child-covered destination ⇒ empty region ⇒ no paint, no damage.

Core plumbing tests (`process_request.rs`, extending
`render_composite_emits_damage_on_dst_drawable` `:40856`): **one test per arm** —
Composite (`:1671`), Trapezoids **and** Triangles (`:1745`), CompositeGlyphs (`:1849`)
— each sets `RecordingBackend.render_return_region` to a known region, drives that op,
and asserts the core damages **exactly** that region (and empty region ⇒ no damage).
Covering all four arms is required: a bad return-capture path in traps/tris/glyphs
must not be able to regress while the composite test stays green. These test the
core-side plumbing independent of the GPU clip logic (tests 1–6 cover the latter).

## Verification

- `cargo fmt`, `cargo clippy` (plain), `cargo test`.
- Paint + damage invariants are proven by the tests above. Because the paint path and
  a backend signature change are involved, a bee/MATE systray + xfce-decoration smoke
  check is a strong confirmation (not a landing blocker per
  `feedback_commit_after_testing`, but worth doing here).

## Files touched

- `crates/yserver-core/src/backend/trait_def.rs` — 4 return-type changes.
- `crates/yserver/src/kms/v2/backend.rs` — `render_dst_cliplist_local` helper +
  4 op bodies (compute, scissor, return) + paint-side tests.
- `crates/yserver-core/src/backend/recording.rs` — configurable return region.
- `crates/yserver-core/src/host_x11/trait_impl.rs` — return empty.
- `crates/yserver-core/src/core_loop/process_request.rs` — 3 arms damage the returned
  region + per-arm core plumbing tests.
- `crates/yserver/tests/v2_acceptance.rs` — mechanical return-type update at existing
  call sites (`:260`, `:385`, `:803`).

## Non-goals / recorded follow-ups

- Fixing the same picture-`subWindowMode`-vs-GC bug in `FillRectangles` (separate).
- Unifying the paint/damage child helpers is now moot for these ops (single region);
  `FillRectangles` still uses the split helpers.
