# The naive occlusion cull broke e16 because a Region silently became its bounding box

**Date:** 2026-09-03. **Branch:** `fix/noncomposited-damage-repaint`.
**Status:** the defect is demonstrated in isolation; that it is what fired on the
hardware run is inferred, not re-measured. It does not need re-measuring, because
the design it dictates for step 1 is the same either way.

## What happened

A "naive step 1" (whole-draw occlusion culling on the Full path, 2026-09-02)
dropped e16's pager thumbnail and made the wallpaper flicker black on Full
frames. It was reverted. Five hardware runs of a diagnostic that logged the
*dropped* draw and the opaque draws *containing* it did not explain the loss,
and the working conclusion became "one of three premises is false": draw order ≠
stacking, `!alpha_passthrough` ≠ overwrites, or dst rects not in one coordinate
space. All three were then verified from source and hold. The thread was left
blocked on a fourth question — whether e16's virtual-desktop windows at `y = ±720`
were a geometry we report wrongly.

Both leads were wrong. The premises hold and the ±720 is e16's own doing. The
draw the cull relied on was not a draw at all: it was the **region cap**.

## The mechanism

`Region::MAX_RECTS` is 32. `enforce_cap` runs after every set operation and,
once the box count *exceeds* 32, replaces the region with **one box: its
bounding rectangle**. That is documented as safe in exactly one direction: a
region that will be *added to damage* may over-approximate; a region that will
be *subtracted* (or, equivalently, used to claim coverage) may not.

The naive cull accumulated a `covered` region — a **union** of opaque dst rects,
walked top-down — and dropped any draw whose rect `covered.contains_rect(...)`.
It guarded the cap with

```rust
if covered.rect_count() >= Region::MAX_RECTS { break; }
```

checked *before* each `add_rect`. That guard only fires if the count lands on
exactly 32. A single `add_rect` of a rectangle that straddles several existing
bands splits each of them, so the count can jump from well under 32 to well over
it in one call — at which point the region has already collapsed to a single
box, `rect_count()` is 1, and the guard can never fire again. From then on
every draw inside that bounding box tests as covered, including draws that
nothing overlaps.

Demonstrated with the branch's `region.rs` verbatim (31 disjoint 10×10 rects in
a column, then one 10×700 rect that crosses all of their bands):

```
after 31 disjoint adds:      rect_count=31
after 1 straddling add:      rect_count=1   bbox=2010x700+0+0
contains_rect(pager at 500,300 — under no draw) = true
guard `rect_count() >= MAX_RECTS`               = false
```

The patch's own cap test (`occlusion_stops_rather_than_trusting_a_capped_region`)
added 40 *disjoint* strips, so the count climbed one at a time, hit 32 exactly,
and the guard fired. It tested the one sequence that cannot collapse.

### Why it matches every symptom

- **Full frames only** — the cull ran only there. Hence "correct during a drag
  (clipped frames), wrong again on release (a Full frame)".
- **e16, not awesome or MATE** — e16's shaped decorations emit hundreds of thin
  overlapping rects per frame (2265 draws per compose at steady state vs MATE's
  56). Overlap is what splits bands; e16 is the scene where one add jumps the
  count.
- **Wallpaper "black / image / black / image"** — once collapsed, the bounding
  box contains the root draw, so the root is dropped and the `loadOp=CLEAR`
  background (black) shows on that Full frame; the next clipped frame repaints
  its damage from the real draw list. Alternation is the two paths interleaving.
- **A 43×41 thumbnail inside a 336×48 pager, nothing overlapping it** — exactly
  the shape of a false positive from a bounding box, and impossible from a real
  cover.

### Why the diagnostics could not see it

`occlusion_diag` explained each drop by listing the individual opaque draws
whose rect contains the dropped one. A collapsed union has **no** such draw, so
those drops printed as `union-only` — which reads as "legitimately covered by
several draws together", the one verdict that looked benign. The dry-run dump's
`region_capped_at` recorded only the explicit `break`, which never happened.
Neither instrument printed `rect_count()` before and after an add, which is the
only place the collapse is visible.

## The e16 desktop windows are not a yserver geometry bug

The single frame that was dumped (`yserver-hw-e16.log`, output at layout
`+2560+0`) has **145 draws**. e16's steady state on this box is 2265 per compose
(median; `docs/superpowers/findings/data/2026-09-02-e16-workload-silence-*.log`).
The dump fired on the first Full frame with ≥ 3 draws, i.e. **during e16's
startup**, and the HEAD screenshot of the steady-state desktop shows none of the
full-width 16-px strips that dump places across the middle of the screen. The
±720 layout was transient.

What the dump shows is consistent with e16's source (`src/desktops.c`,
Debian `e16` 1.0.0-4):

- Desks are full-`VROOT`-size siblings under the root
  (`EoInit(dsk, EOBJ_TYPE_DESK, win, 0, 0, WinGetW(VROOT), WinGetH(VROOT), ...)`),
  so `5120×1440` on this two-output layout — matches `0x400006` and `0x400008`.
- Each desk carries a full-size **child** `Desk-bg-N` created at the desk's
  origin (`EobjReparent(eo, EoObj(dsk), 0, 0)`). The dump's `0x400007` and
  `0x400009` are `5120×1440` children clipped by their parent to
  `5120×720` at absolute `(0,0)`, i.e. counter-positioned so the wallpaper stays
  put while the desk moves — matches.
- **jos identified the animation (2026-09-03): at startup e16 opens the desktop
  like a shutter, from the middle out to the top and bottom.** Two desks at
  exactly `y = −720` and `y = +720`, each showing the correct half of a
  stationary wallpaper through its counter-positioned background child, is
  that shutter at its midpoint. Desk slides in general
  (`DeskGoto` with `Conf.desks.slidein`: `DeskMove(dsk, 0, ±WinGetH(VROOT))`
  then `EobjSlideTo(..., 0, 0)`) and drag-bar drags move desks the same way. A
  hidden desk sits at `x = WinGetW(VROOT)`, fully off-screen — never at a
  half-screen offset.
- The dragbars (`0x40000a`/`0x40000b`, `5120×16`) sit at the desks' edges
  (`y = 704` and `720`).

So on that frame the two desks legitimately tile the output, and everything the
dump lists below them — including the pager `0x40005c` at the bottom-right of
the *right* monitor — is covered on Xorg too. That is **not** the pager jos
watched (`0x40009c`, bottom-left of the *left* monitor, whose output was never
dumped). The "pager content dropped, contained by the desk windows" reading
conflated two different pagers, on two different outputs, in a frame that was
not the steady state.

An x11trace of e16 startup shows it as a `ConfigureWindow` sequence for
`0x400006`/`0x400008` walking from the centre outwards and ending at `y = 0`
or off-screen. Confirmatory only; not a gate for step 1.

## What this dictates for step 1

1. **Never claim coverage from a union.** Any region used to decide "this is
   hidden" or "this is covered" must be built by *subtracting* from a
   remainder, so that a cap collapse yields a superset of what is still visible
   — under-culling, never a hole. wlroots tracks the remaining-visible region
   for exactly this reason. This applies to the visibility pass **and** to step
   4's opaque-cover gate once the root is no longer a single rect.
2. **Clip, don't drop.** Whole-draw rejection is the degenerate case of clipping
   a draw to its visible region; clipping is what turns the reporter's
   full-output root under a near-full-screen terminal into a thin frame of
   rects, which whole-draw rejection cannot touch.
3. **Keep painter's order.** With a capped (superset) visible region a lower
   draw may paint pixels a higher one also paints; that is harmless only
   because the higher draw is recorded later. Emission order stays parent →
   children, siblings bottom → top.
4. **Instrument the cap.** Count collapses per frame. A collapse is not a bug,
   but a scene that collapses every frame is one where the pass buys nothing.

The design is in the plan,
`docs/superpowers/plans/2026-09-01-damage-derived-scene-repaint-plan.md`, step 1.

## Incidental, not chased

- **Full frames with more than 1024 draws render truncated.** Descriptor sets
  come from a pool of `MAX_DESCRIPTOR_SETS_PER_FRAME = 1024`
  (`vk/pipeline.rs:119`); allocation `break`s at exhaustion and
  `record_command_buffer` takes only the allocated prefix, so the **topmost**
  draws are the ones missing. e16 runs 2265–2509 draws per compose at steady
  state; no `descriptor allocation failed` appears in any e16 log only because
  steady-state Full frames are zero. Step 1 makes the list shorter (occluded
  windows emit nothing) but does not bound it. Separate issue.
- `emit_window_subtree` scans the whole `WindowsMap` per node to find children
  (O(N²)); the visibility pass needs a children index anyway, so build one per
  `build_scene`.
