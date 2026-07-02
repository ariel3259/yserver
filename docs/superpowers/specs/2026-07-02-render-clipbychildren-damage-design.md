# RENDER ClipByChildren damage — Composite / CompositeGlyphs / Trapezoids

**Date:** 2026-07-02
**Branch:** `feat/render-clipbychildren-damage`
**Survey item:** `[T1]` in `docs/superpowers/findings/2026-06-26-stub-gap-survey.md` — "RENDER
`ClipByChildren` on Composite/CompositeGlyphs/Trapezoids."

## Goal

Stop three RENDER paint paths from over-reporting damage across mapped child
windows. Today Composite, CompositeGlyphs, and the Trapezoids/Triangles family
each damage the **whole destination drawable** after painting; X.Org clips RENDER
damage to the destination's `clipList` (its geometry **minus** its mapped
`InputOutput` children — the default `ClipByChildren` subwindow mode). The
over-damage is the same class that drove the mate-panel systray recomposite
self-loop (~485 ops/frame) that was fixed for `FillRectangles` in `e7a1ba0`; these
three ops are the documented follow-up.

## Scope (Approach 1 — damage-clip only)

Chosen after weighing three approaches (see "Alternatives considered"). This
change clips **damage**, not paint.

### The exact change

At each of the three RENDER arms in
`crates/yserver-core/src/core_loop/process_request.rs`, replace the damage call

```rust
let _dropped = accumulate_damage_full_to_state(state, dst_drawable);
```

with

```rust
let _dropped = accumulate_damage_clip_by_children_to_state(state, dst_drawable);
```

| Op (RENDER minor) | Arm | Damage call today |
|---|---|---|
| Composite (8) | `process_request.rs:1628` | `:1671` |
| Trapezoids/Triangles/TriStrip/TriFan (10–13) | `:1675` | `:1745` |
| CompositeGlyphs8/16/32 (23–25) | `:1806` | `:1849` |

`accumulate_damage_clip_by_children_to_state`
(`crates/yserver-core/src/core_loop/damage_fanout.rs:181`) already exists and is
the exact helper `FillRectangles` (op 26, `:1899`) uses on its damage side. No new
helper, no signature change, no backend change. The paint calls
(`backend.render_composite` / `render_composite_glyphs` / `render_trapezoids` /
`render_triangles_op`) are untouched.

Op 22 (FreeGlyphs) emits no damage and is correctly left alone.

## Correctness rationale

- The helper damages `drawable_full_rect − mapped_InputOutput_children`. When the
  painted region does not overlap a mapped child — the overwhelmingly common case,
  painting into a window's own visible area — **no child intersects, so damage is
  reported in full**: zero behavioural change, no regression risk.
- It only *reduces* damage where a mapped child actually overlaps the destination —
  precisely where X.Org's composite clip is empty and it emits no damage. So the
  change is strictly-more-correct and never under-reports the common case.
- For **pixmap** destinations the helper is identical to the full variant (pixmaps
  have no children), so pixmap paints are entirely unaffected.
- Consistency: this mirrors the `FillRectangles` precedent exactly. That path's
  damage side also uses the whole-drawable-minus-children helper (it does **not**
  tighten damage to the painted sub-rect either), so we adopt the established
  damage model rather than inventing a new one.

## Non-goals (the Approach-1 boundary — stated explicitly, not stubbed)

- **Paint is not clipped.** In the shared-backing case (a non-redirected child
  paints into a redirected ancestor's backing, which the scene emits once —
  `scene.rs:2155-2163`), a parent Composite/Glyph over-paint into a child-covered
  region is left as-is. Unlike the `FillRectangles` `Clear` case, there is **no
  demonstrated visible bug** for these three ops over-painting child windows (real
  toolkits clip client-side and rarely RENDER-paint over child windows), so
  paint-clipping here would be speculative (YAGNI). If a shared-backing over-paint
  bug is ever observed, paint-clip is a clean, scoped follow-up.
- **Damage precision to the painted sub-rect** is not added; the whole-drawable
  extent (minus children) is inherited from the existing helper, matching
  `FillRectangles`.

## Testing (TDD — red first)

Mirror the existing harness `render_composite_emits_damage_on_dst_drawable`
(`process_request.rs:40856`): `ServerState::new()` + `RecordingBackend` +
`install_client` + `create_window` + `create_picture` + a `DamageObject`
subscription on the destination window, a hand-built wire body, then
`process_request`, then assert on `state.damage_objects[..].rects`.

Extend it into three focused tests (one per op family). Each:

1. Creates the 800×600 destination window **plus a mapped `InputOutput` child**
   covering a known sub-rect (e.g. `100,80 200x150`).
2. Drives the RENDER request (Composite / CompositeGlyphs / Trapezoids) with a
   destination region that overlaps the child.
3. Asserts the resulting `damage.rects` equal **window-minus-child**, not the full
   window.

Reference the rect-shape assertions in
`clip_by_children_partial_cover_damages_only_margin`
(`damage_fanout.rs:1361`) for the expected minus-child region decomposition.

**Each test must go red against the current `accumulate_damage_full_to_state`
code first** (it will report the full 800×600, including the child region), then
green after the one-line swap. Per project practice, prove red before applying the
fix; do not bundle test + fix in one step.

Also add a childless-window / pixmap-destination case (or assert within an existing
one) to lock in the "no child → full damage, no regression" invariant.

## Verification

- `cargo fmt`, `cargo clippy` (plain — pedantic is opt-in here), `cargo test`.
- This is a damage-invariant change proven by unit/integration tests; per
  `feedback_commit_after_testing`, a green test pass is commit-worthy on its own —
  no interactive smoke gate required to land it. An opportunistic bee/MATE systray
  smoke check is a nice-to-have, not a blocker.

## Files touched

- `crates/yserver-core/src/core_loop/process_request.rs` — three-line swap +
  three new tests (extending the `render_composite_emits_damage_on_dst_drawable`
  test module).
