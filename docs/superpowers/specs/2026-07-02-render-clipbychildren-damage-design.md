# RENDER ClipByChildren parity — Composite / CompositeGlyphs / Trapezoids / Triangles

**Date:** 2026-07-02
**Branch:** `feat/render-clipbychildren-damage`
**Survey item:** `[T1]` in `docs/superpowers/findings/2026-06-26-stub-gap-survey.md` — "RENDER
`ClipByChildren` on Composite/CompositeGlyphs/Trapezoids."

## Goal

Give four RENDER paint ops full X.Org `ClipByChildren` parity — on **both** the
paint and the damage side. X.Org clips RENDER paint, and the damage it generates,
to the destination window's `clipList` = its geometry MINUS its mapped children
(the default `ClipByChildren` subwindow mode). yserver currently does neither for
Composite, CompositeGlyphs, and the Trapezoids/Triangles family: it paints the full
destination (stomping any shared-backing child pixels underneath — a divergence
from X.Org) and damages the full destination drawable (the over-damage class that
drove the mate-panel systray recomposite self-loop, ~485 ops/frame, fixed for
`FillRectangles` in `e7a1ba0`).

This closes both halves for all four ops, matching what `FillRectangles` already
does (`FillRectangles` clips paint via rect subtraction and damage via
`accumulate_damage_clip_by_children_to_state`).

### Why Approach 2 (full parity), not damage-only

A damage-only change (originally scoped as "Approach 1") was rejected after a codex
review: clipping damage without clipping paint leaves the paint free to overwrite a
non-redirected child's pixels in a **shared backing** (`scene.rs:2155-2163`: a
non-redirected child paints into its redirected ancestor's backing, which the scene
emits once — the child does not re-composite on top). Damage-only is *paint-neutral*
(it introduces no new corruption — the paint divergence already exists on master),
but it does not *fix* that divergence and cannot claim X.Org parity. Since the
paint-clip turns out to be cheap here (below), we do it properly.

## The change is small because the machinery already exists

The v2 KMS backend/engine already carry a per-op clip-rect list end-to-end and draw
each op through a **multi-rect scissored loop**:

- Each op computes `dst_clip` (the client's picture clip) via
  `resolve_dst_picture_for_render` → `shift_dst_picture_clip`, then hands it down to
  `build_render_clip_scissors` (`engine.rs:10906`) → per-rect
  `cmd_set_scissor` draws (`ops/render.rs:327`, `ops/text.rs:189`). This runs today
  for client-set clips (`SetPictureClipRectangles`, op 6).
- A ready-made ClipByChildren primitive exists:
  `clip_fill_rects_by_subwindow_mode` (`backend.rs:7130`) — subtracts every mapped
  **automatic** child window and **correctly skips manually-redirected children**
  (they own their own backing and composite on top, so painting under them is not
  visible corruption).
- A proven end-to-end template exists: `compute_copy_area_scissors`
  (`backend.rs:2411`) already does "GC clip ∩ ClipByChildren child subtraction →
  scissors" for CopyArea, including the destination-offset coordinate shift.

So paint-clip = intersect the existing `dst_clip` with the child subtraction before
it flows into the scissor builder. No `Backend` trait signature change, no engine
change, no change to the recording/host_x11 backends.

## Paint side — four symmetric edits in `crates/yserver/src/kms/v2/backend.rs`

| Op | `fn` | dst_clip computed at |
|---|---|---|
| Composite (minor 8) | `render_composite` `:15151` | `:15201` (`shift_dst_picture_clip`) |
| CompositeGlyphs (23–25) | `render_composite_glyphs` `:15327` | `:15409` |
| Trapezoids (10) | `render_trapezoids` `:15806` | `:15881` |
| Triangles/TriStrip/TriFan (11–13) | `render_triangles_op` `:15988` | `:16083` |

The child-clip must run in **dst-window-local** coordinates — that is what
`clip_fill_rects_by_subwindow_mode` (and the `windows_v2` child geometries)
expect — so it is inserted **before** `shift_dst_picture_clip` applies
`dst_target.offset` (which converts local → backing coords). At each site, when
`current_subwindow_mode == ClipByChildren` and the destination is a window:

1. **Base region (local coords).** Take the pre-shift picture clip from
   `resolve_dst_picture_for_render` (already local). If it is `Some(rects)`, use
   those; if `None` (client set no clip → unbounded), synthesize the full
   dst-window-local extent as a single rect (there must be a base region to subtract
   children from).
2. **Subtract children (local coords).** Pass the base region through
   `clip_fill_rects_by_subwindow_mode(dst_host_xid, base)`, which returns the
   child-subtracted survivors (it skips manually-redirected children internally).
3. **Shift to backing coords.** Run the result through the existing
   `shift_dst_picture_clip(..., dst_target.offset)`, exactly as `dst_clip` is shifted
   today. Because the subtraction happened in local space, no child rects need
   manual offsetting — the single existing shift covers everything. (This ordering
   is what makes the "coordinate wrinkle" disappear; `compute_copy_area_scissors`
   at `:2411` is the reference for the local-subtract-then-shift pattern.)
4. Feed the shifted rects into the existing `dst_clip` path unchanged. If the result
   is empty (destination fully covered by children — the fully-covered systray
   socket), the op paints nothing, matching X.Org's empty-clip no-op.

Because all four bodies are structurally identical through the `dst_clip` step, a
shared private helper
`clip_render_dst_by_children(dst_host_xid, pre_shift_clip) -> Option<Vec<Rectangle16>>`
factors steps 1–2 (local-space child subtraction + full-extent synthesis) so each op
is one call inserted just before its existing `shift_dst_picture_clip`, and the
subtraction logic is tested once. The offset shift stays the op's existing line.

## Damage side — three edits in `crates/yserver-core/src/core_loop/process_request.rs`

Swap `accumulate_damage_full_to_state(state, dst_drawable)` →
`accumulate_damage_clip_by_children_to_state(state, dst_drawable)` at:

| Op (RENDER minor) | Arm | Damage call today |
|---|---|---|
| Composite (8) | `:1628` | `:1671` |
| Trapezoids/Triangles/TriStrip/TriFan (10–13) | `:1675` | `:1745` |
| CompositeGlyphs8/16/32 (23–25) | `:1806` | `:1849` |

This is the exact helper `FillRectangles` (op 26, `:1899`) uses. Op 22 (FreeGlyphs)
emits no damage and is left alone.

## Correctness notes (accurate — supersedes the earlier draft)

- **Damage behaviour changes for any window with a mapped child, not only when the
  paint overlaps a child.** `accumulate_damage_clip_by_children_to_state`
  (`damage_fanout.rs:181`) subtracts child rects from the drawable's **full extent**
  regardless of where the paint landed. This is still safe: the painted area is
  always reported (a paint that misses every child is unaffected by the subtraction);
  we only stop reporting the child-covered region, which was already stale
  over-damage. It matches the `FillRectangles` precedent, which also damages
  whole-drawable-minus-children rather than tightening to the painted sub-rect. (This
  corrects the earlier spec's inaccurate "no change in the common case" claim.)
- **Pixmap destinations** have no children, so both the paint helper (no
  `windows_v2` entry) and the damage helper are identity — pixmap paints are
  unaffected.
- **Paint/damage child-set asymmetry (known, low-impact, inherited).** The paint
  side (`clip_fill_rects_by_subwindow_mode`, `windows_v2`-based) subtracts mapped
  *automatic* children and **skips manually-redirected** ones; the damage side
  (`mapped_child_clip_rects`, resources-tree-based) subtracts all mapped
  `InputOutput` children with `map_state != Unmapped` (so it also subtracts
  manually-redirected and `Unviewable` children). The two helpers therefore compute
  slightly different child sets. This asymmetry already exists for `FillRectangles`
  and is low-impact (a manually-redirected child composites its own backing on top,
  so slightly-different damage under it only affects whether an occluded region is
  needlessly repainted; `Unviewable` direct children of a viewable dst do not occur
  in practice — `Unviewable` requires an unmapped ancestor). We **do not** unify the
  two helpers in this change (that would also alter `FillRectangles` behaviour); it
  is recorded as a follow-up. The load-bearing property — paint never stomps a
  *visible* shared-backing child — is satisfied by the paint helper's skip rules.

## Testing (TDD — red first, both sides)

Backend paint-side tests (`crates/yserver/src/kms/v2/backend.rs` test module,
mirroring `clip_fill_rects_by_subwindow_mode_*` and the
`copy_area_clip_by_children_*` tests):

1. Per op (composite / glyphs / trapezoids / triangles): a dst window with a mapped
   automatic child overlapping the paint yields scissor rects = dst-region-minus-child.
   Red against current code (full-dst scissor), green after the edit.
2. **Coordinate-space test:** a redirected dst (non-zero `dst_target.offset`) with a
   child — assert the subtracted child rects are shifted by the offset (guards the
   step-3 wrinkle). This is the test most likely to catch a real bug.
3. Manually-redirected child is **not** subtracted from paint (reuses the
   `copy_area_clip_by_children_skips_manually_redirected_child` fixture shape).
4. Fully-covered destination → empty clip → op paints nothing.

Damage-side tests (`process_request.rs` test module, extending
`render_composite_emits_damage_on_dst_drawable` `:40856`):

5. Per op: dst window + mapped `InputOutput` child overlapping the paint →
   `damage.rects` = window-minus-child (ref the rect decomposition in
   `clip_by_children_partial_cover_damages_only_margin`, `damage_fanout.rs:1361`).
   Red against `accumulate_damage_full_to_state`, green after the swap.
6. No-child / pixmap destination → full damage (no-regression invariant).

Codex's Finding 4 (a damage-rect assertion cannot prove the backing isn't
corrupted) is addressed by tests 1–4, which assert on the **paint** scissor rects —
the actual GPU-clip decision — not just the damage list.

## Verification

- `cargo fmt`, `cargo clippy` (plain — pedantic is opt-in here), `cargo test`.
- Damage + paint invariants are proven by the unit tests above; per
  `feedback_commit_after_testing` a green test pass is commit-worthy on its own.
  A bee/MATE systray + xfce-decoration smoke check is a valuable confirmation here
  (paint path changed, shared-backing case is live) but not a landing blocker.

## Files touched

- `crates/yserver/src/kms/v2/backend.rs` — one shared `clip_render_dst_by_children`
  helper + four one-line call sites + paint-side tests.
- `crates/yserver-core/src/core_loop/process_request.rs` — three-line damage-helper
  swap + damage-side tests.

## Non-goals / recorded follow-ups

- Unifying the paint-side and damage-side child-set helpers (see asymmetry note).
- Tightening damage to the exact painted sub-rect (kept at
  whole-drawable-minus-children for `FillRectangles` consistency).
