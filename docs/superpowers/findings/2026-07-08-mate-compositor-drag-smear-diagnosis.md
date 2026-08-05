# MATE compositor drag smear — diagnosis & fix

**Date:** 2026-07-08
**Status:** ✅ **RESOLVED + HW-CONFIRMED** — fix in `master 0d9972ad`
(`fix(copy_area): correct negative-offset clamp`).
**Repro:** was easy on bee AND silence; present for weeks (NOT introduced by
`perf/composite-copy-fastpath`; bisected back to ≥ v1.1.1). MATE with **compositing on**.

## RESOLUTION (confirmed)
Root cause was a **negative-offset double-subtract** in `engine::copy_area` /
`masked_copy_area`. The COW present copies only the compositor's `update` region;
dragging left/up past the screen edge makes those rects have a **negative origin**
(the `PRESENT-UPDATE` trace showed `x0=-10`, `x1` up to 3048 > 2560). The copy path
clamped the *source* with `clamp_rect` — which already trims width/height for a
negative offset — and then **re-subtracted** the negative `dst_pos` in the copy-size
formula, under-copying by `|offset|` px on the **trailing** edge. Those un-refreshed
COW strips kept stale content; the drop-shadow gradient made the 1-px+ gaps visible.
Direction-specific because positive offsets are arithmetically identical
(`dst_pos.x == dst_pos_clamped.x`) — only negatives double-count.

**Fix:** one shared `clamp_copy_rects(src_rect, dst_pos, src_extent, dst_extent)`
helper (engine.rs, beside `clamp_put_rect`) that jointly clamps src+dst in a shared
index space, aligned, with one shared extent; both `copy_area` and `masked_copy_area`
use it. Guarded by red→green unit tests (`engine::tests::clamp_copy::*`) + a lavapipe
GPU test (`copy_area_negative_offset_copies_trailing_strip`).

**HW result:** killed the slow-drag shadow smear AND a separate long-standing bug —
fast-drag leaving window bits on the background *when the window touches the left
edge* (same negative-x trigger). Two symptoms, one root cause.

> Note on this doc's original hypotheses (below): the *localization* (Present→COW
> copy) was correct, but the leading sub-hypotheses (ClipByChildren shaving,
> compositor damage-tiling gaps) were **wrong** — the `YSERVER_PRESENT_TRACE`
> instrumentation (this branch) is what revealed the negative-origin update rects and
> pointed at the clamp arithmetic. Kept below as an honest record of the hunt.

---

## Original localization notes (pre-fix)

**Status at the time:** Root cause LOCALIZED with hard pixel evidence; exact
sub-mechanism still to confirm. No fix attempted yet.

## Symptom
Dragging a window **slowly** over the desktop / another window leaves faint
**ghost/streak trails** in the swept region (windows *and* sometimes the desktop
background). Trails clear when that region is later repainted (hover/focus).
**Fast dragging does NOT reproduce** — the key discriminator.

## Evidence (SIGUSR1 dump, 2026-07-08 21:19, in repo root)
Pipeline confirmed from `yserver-v2-drawable-0-windows.txt`: MATE compositor
redirects all top-levels, composites into **double-buffered pixmaps** `0x40002a` /
`0x400034`, and `Present`s **full-screen (2560×1440)** frames to the **Composite
Overlay Window `0x103`** (COW, → `DrawableId(28)`). yserver scans out the COW.

Three-way pixel comparison of the smeared right window region (caja "jos" at
1319,173, 967×723) — see `scratchpad/cmp_cow_vs_psrc31.png`:

| Image | Right-window region |
|---|---|
| `present-src-31` (compositor's newest presented pixmap) | **CLEAN** |
| `cow-0x103` (COW backing) | **STREAKED** |
| `scanout-0-out0` (displayed) | **STREAKED** (matches COW) |

Conclusions from the pixels:
1. **The compositor's output is correct** — the presented source pixmap is pristine.
2. **The COW backing is wrong** — stale streaks in a sub-region.
3. **scene compose is innocent** — scanout faithfully equals the COW (compose is
   `Repaint::Full`, so it just shows whatever the COW holds; it does not add the
   streak).
4. COW's streaked pixels match **none** of the last 10 presented sources
   (`compare -metric AE` identical vs psrc 28/29/30/31 → the stale content predates
   the present ring). So it is old un-refreshed content, not a recent-frame swap.

⇒ **The bug is in the `PresentPixmap` → COW copy path**, not the compositor and not
scene compose.

## Mechanism (high confidence)
The present-copy handler (`crates/yserver-core/src/core_loop/process_request.rs:7814-7854`)
copies **only the `update` region rects** from the source pixmap into the COW
(`backend.copy_area` per rect), leaving the rest of the COW untouched. This is
spec-correct (matches Xorg `present_execute_copy`) **iff the COW retains the previous
frame's content in the un-updated region**. On Xorg the persisted content is the
correct last frame → no smear. On yserver the un-refreshed strips hold **wrong**
content → smear.

The **slow-vs-fast** discriminator pins it to **thin update rects**:
- Slow drag → per-frame damage is a thin sliver → yserver leaves un-refreshed
  strips in the COW → faint streaks (the strips between what actually got copied).
- Fast drag → damage is a fat rect covering the whole swept area → fully overwrites
  → any gap is painted over → no visible smear.

"Sometimes on the desktop bg" = the swept trailing background, same mechanism.
"Clears on hover/focus" = a later, larger damage present fully repaints that strip.

**Why it's VISIBLE = the drop-shadow gradient (user hypothesis, corroborated).** The
trails are faint grey — the character of a compositor drop-shadow's low-alpha
gradient, not opaque window content. The observed streaks sit where a *neighbouring*
window's shadow sweeps (e.g. the left window `0x4012fc`'s right-edge shadow ~x=1436+
crossing the right window's Name column ~1349–1476), and the shadow extends beyond
frames onto the wallpaper (hence "sometimes on the bg"). A smooth gradient turns a
1-px un-refreshed strip into a visible discontinuity; the same stale strip over solid
opaque content would be invisible. This does NOT change the localization: the shadow
is already baked into *opaque* pixels in the (clean) present source, so the COW copy
is still copying opaque data — the gradient only governs *visibility* of the
under-refreshed strips.

## Leading sub-hypotheses for the defect INSIDE the COW copy (confirm tomorrow, pick ONE, test minimally)
1. **Thin-rect copy is shaved/dropped.** `copy_area` into the COW binds a default
   `DrawState` (clip=None) but leaves `subwindow_mode = ClipByChildren`
   (`process_request.rs:7798-7813` fixed the clip=None hazard but not this). The
   v2 `copy_area` ClipByChildren step (`crates/yserver/src/kms/v2/backend.rs:2618-2653`)
   subtracts mapped child windows from the destination — check whether, for a COW
   destination, it shaves/ō drops thin present-update rects (edges, or slivers that
   round to empty). NOTE: only the RIGHT window + swept strips smear, not the left
   window → it is NOT a blanket "all window regions masked", which argues for a
   thin-rect/edge effect rather than wholesale subtraction.
2. **Update-region coordinate space.** Copy uses `src=(rect.x,rect.y)`,
   `dst=(x_off+rect.x, y_off+rect.y)` treating the update rect as pixmap-relative;
   Present spec update region is **window-relative**. Harmless when `x_off=y_off=0`
   (full-screen COW present, the case here) — so probably NOT the cause here, but
   verify x_off/y_off are actually 0 in the trace.

   **DISPROVEN (2026-08-01, Task 13).** The update region is
   **pixmap-relative**, not window-relative — settled against the Xorg
   source during the successor-gate-relaxation amendment review: Xorg
   installs the update region as a `CT_REGION` clip with
   `GCClipXOrigin/GCClipYOrigin = x_off/y_off` and copies the whole
   pixmap to `(x_off, y_off)` (`~/Projects/xserver/present/present.c:76-92`),
   so source pixel `s` survives iff `s ∈ region` — region coordinates
   are pixmap coordinates. yserver's `x_off.saturating_add(rect.x)`
   copy-arm arithmetic matches Xorg exactly. See spec
   `docs/superpowers/specs/2026-07-31-present-deferred-execution-supersession-design.md`
   §"Amendment 2026-08-01 — successor-gate relaxation" ("Coordinate-space
   resolution" paragraph) for the full argument.
3. **COW backing does not persist / rotates.** Verify the COW (`DrawableId 28`) is a
   single persistent storage image that `copy_area` accumulates into, not something
   re-derived/rotated per present that could lose un-updated strips.

## NOT an alpha-mask/blend off-by-one — it's a COVERAGE off-by-one
Tempting hypothesis (raised + rejected): a ±1 in an alpha mask/blend. Rejected on
evidence: the Present→COW step is `copy_area` = straight `vkCmdCopyImage` (no blend;
shadow is already baked opaque into the clean source). The signature is **staleness**
(un-refreshed strips older than the 10-frame ring; clears on a later covering
present; fast drag does NOT repro) — a blend ±1 would mis-value **every** frame incl.
fast drag, not leave stale strips. The likely ±1 is in **update-rect coverage
geometry**: rect width/height, inclusive-vs-exclusive bound, or the ClipByChildren
clip-intersection leaving a **1-px un-copied strip** per slow-drag frame. Thin
slivers (slow drag) + 1-px shortfall → 1-px stale gaps; the shadow **gradient** is
merely what makes a 1-px gap visible (opaque content would hide it).

## Next experiment (diagnostic-first, per the perf-branch lesson)
Instrument `handle_present_request` (the `req.update != 0` branch) to log, per present
during a slow drag: the `req.update` rect list (count + geometry), `x_off/y_off`, and
the rects `copy_area` **actually applied** after clip/ClipByChildren. Then check
whether the union of applied rects fully tiles the swept region or leaves the
observed strips. Compare the same trace shape against Xorg `present_execute_copy`
(`xserver/present/present_execute.c:101-136`) — Xorg is the de-facto spec.

## Repro recipe
MATE + compositing on, drag a window **slowly** over another window / desktop; watch
for streak trails in the vacated strip. `SIGUSR1` to dump COW + present-src ring +
scanout, then compare `cow-*` vs newest `present-src-*` in the same region.

## Cross-refs
- `project_resize_flicker`, `project_i3_float_drag_smear` (FIXED `ee4ed590`,
  configure-damage gated on Viewable — different path: non-composited),
  `fix/configure-move-uncovered-damage` branch (non-composited move damage).
- This is a **compositing/COW-present** smear, a distinct path from those.
- NOT the same as the disabled buffer-age partial-repaint (#6 in
  `2026-07-08-xorg-render-optimization-gaps.md`) — that's scene compose, proven
  innocent here.
