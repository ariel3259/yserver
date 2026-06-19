# Handoff: click-below / nemo-rises (cinnamon, sloppy focus) — RESOLVED

Date: 2026-06-20 (air, Asahi HW). Branch: **fix/focus**.
Supersedes the 2026-06-19 version. The earlier diagnoses in this file's history
(sloppy-focus Enter loop; missing hierarchy-change crossings; dual-authority
stacking) were all **wrong tracks** — see "Dead ends" below.

## Repro

Two overlapping file managers, **nautilus (GTK4, frameless) on top of nemo
(GTK3, reparented into a muffin frame)**, cinnamon focus-mode = sloppy. Click
inside nautilus far from edges. Historical symptoms: the click landed in / raised
**nemo** (the window below).

## Root cause (HW-confirmed, two independent bugs)

### 1. yserver never emitted `ShapeNotify` — the primary bug
yserver tracked `ShapeSelectInput` subscribers (`shape_select_masks`) but **never
sent `ShapeNotify`** when a window's shape changed (`grep -c ShapeNotify` in
muffin's wire = 0). Sequence, from `cinnamon.xtrace`:
1. nautilus (GTK4/CSD) sets a tiny **startup input shape** `{13,13,24,24}`.
2. muffin `ShapeSelectInput`s + `GetRectangles` → **caches 24×24**.
3. nautilus grows the input shape to the real `{13,13,1482,1024}`.
4. yserver applies it to its own store (so yserver's hit-test stays correct and
   delivers the click to nautilus) **but sends no `ShapeNotify`** → muffin's
   clutter reactive region for nautilus stays **24×24**.
5. A click outside the stale 24×24 (but inside the real window) → muffin's
   clutter pick finds nautilus click-through there → **falls through to nemo** →
   focuses + raises nemo.

So muffin *rendered* nautilus on top (scanout confirmed) but *hit-tested* most of
it as a hole. `ShapeNotify` was never implemented — a missing feature, not a
regression.

### 2. `gen_hit` used frame-relative coords for framed windows — branch regression
The click-below commit (`8e5ca7a`) added `resolve_pointer_hit` with a **`gen_hit`**
path for button events that fed `event.event_x/event_y` into
`pointer_target_at(tl, …)` as if they were local coords within `tl`. For a window
reparented into a WM frame, `event_x/y` is relative to the window's
*parent-relative* origin, not its *screen-absolute* origin, so clicks on framed
**nemo** landed ~50px low / right. (Master has no `gen_hit`; it hit-tests clicks
*and* motion via `root_pointer_target_at`, so they stay consistent — this bug is
branch-only.)

## The fix (committed)

1. **`ShapeNotify` emission** (`yserver-protocol` `encode_shape_notify_event`;
   `yserver-core` `emit_shape_notify` called after RECTANGLES/MASK/COMBINE/OFFSET
   in `handle_shape_request`; `nested::SHAPE_FIRST_EVENT` made `pub(crate)`).
   Test: `shape_input_change_emits_shape_notify_to_selectors`.
2. **`gen_hit` coords** (`pointer_fanout::resolve_pointer_hit`): derive local
   coords as `root_x/y − window_absolute_position(tl)` instead of using
   `event_x/y`. Keeps the click-below "target locked at event generation"
   benefit; no-op for top-levels (absolute == parent-relative); puts clicks on
   the same root-based basis as motion.

**Master-shared code is untouched** (`event_relative_coords`,
`translate_host_event` left as master has them). 821 core + 521 yserver tests
green; nightly fmt clean.

## Status

HW-confirmed by user 2026-06-20: nautilus click stays on nautilus, nemo no longer
rises, framed-nemo clicks land correctly, hover highlight lines up. User will do a
**broader morning pass** to confirm no regressions across other apps / framed WMs.

## Dead ends (do not re-try)

- **Sloppy-focus Enter(nemo) loop** (old framing): wrong — trigger is a
  ButtonPress correctly delivered to nautilus.
- **Missing hierarchy-change crossings** on configure/map/unmap/restack: real
  spec gap, but NOT this bug; the synthesized crossings were *spurious*
  (`pointer nemo → cinnamon-overlay`) and broke nemo clicks. Reverted.
- **Button-press `update_pointer_window` reconcile**: always `SKIP-SAME` here;
  dead weight. Reverted.
- **Patching `event_relative_coords` / `translate_host_event`** (master-shared)
  to compensate: wrong layer — the regression is the branch-only `gen_hit`.
  Reverted. **Lesson: `git log master..HEAD` + `git show master:<file>` before
  "fixing" coordinate/stacking logic on this branch.**

## Tooling

- `just yserver-cinnamon-hw` (logs clickhit/restack/focus + scene). For pointer
  crossing debugging, temporarily add `yserver::kms::v2::pointer=trace` to the
  recipe's log filter (shows `upw:` / `dispatch_motion`).
- `just yserver-cinnamon-hw-trace` writes `cinnamon.xtrace` (muffin's wire) — the
  decisive artifact for "what muffin reacts to" (e.g. `ShapeNotify` count, SHAPE
  Input rects).
- Scanout dump: **Ctrl-Alt-F12** → `yserver-v2-scanout-*.ppm`. Convert + crop at
  the click coords (`magick … -crop`) to see what's *rendered* under the cursor —
  this is what disambiguated "muffin pick wrong" vs "yserver stack wrong".
