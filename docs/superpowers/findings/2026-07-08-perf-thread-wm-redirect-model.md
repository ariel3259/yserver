# Perf thread outcome: WM redirect model kills top-level scanout/occlusion (#7 + occlusion cull)

**Date:** 2026-07-08. **Machine:** silence (i9-13900K / RX 580, amdgpu+RADV), dual 2560×1440.

## TL;DR
Chasing "choppy / dropped-frame fullscreen video" led to two optimization attempts —
**direct-scanout page-flip (#7)** and **top-level occlusion culling** — and HW measurement
killed **both for the realistic (compositing-desktop) case**, for the *same* reason. The
actual finding is reassuring: **there was no yserver present-path bug.** Under a
well-behaved compositing WM (cinnamon) yserver runs **dual fullscreen video at 100–119 Hz
with zero dropped frames**. The "choppy" was WM-specific (fvwm load; e27 quirks).

## The load-bearing fact: `participating = 0` on every compositing WM
The scene compositor draws per-window; the fullscreen-bypass (`suppress_cow`), the
occlusion cull, and compositor-side direct scanout all key on a window being
**`scene_participating`** (unredirected, drawn directly by yserver). HW `scene`-debug
telemetry with a fullscreen app, across **fvwm(+composite), e27, cinnamon**:

```
participating=0   covers=1..2   participating_covers=0   highest_full_occluder_idx=None
```
(293k+ samples on e27; identical shape on cinnamon.) Every compositing WM **redirects**
client windows — the fullscreen app is a **child under the COW** (compositor overlay
window), not a participating top-level. yserver draws it via the COW subtree recursion
(~40–50 draws/frame). The top-level walk never sees an unredirected covering window.

Consequences:
- **#7 direct scanout is inapplicable.** Both routes (X Present client-flip; compositor
  backing-flip) need an unredirected output-covering buffer. There isn't one. (M1 *did*
  prove the RX 580 imports a client dma-buf as a scanout FB — capability real, target
  absent. Branch `feat/direct-scanout-m1`, parked.)
- **Top-level occlusion culling never fires.** The covering window is in the COW subtree,
  not a top-level; the cull inspects top-levels. Correct + TDD'd but inert on compositing
  desktops. Branch `feat/occlusion-cull`, unmerged/dead.

## What WOULD be needed (not pursued)
- Occlusion/scanout would have to operate **inside the COW subtree** (cull redirected
  windows occluded by the fullscreen redirected app) — a much larger, subtler change in
  the redirect path.
- OR a WM that genuinely **unredirects fullscreen** into a participating window. mutter/
  muffin are *reputed* to (`unredirect_fullscreen`), but in yserver's model cinnamon still
  showed `participating=0` — so either they don't here, or yserver doesn't mark the
  unredirected window participating. Untested lever.
- Buffer-age **partial repaint** (make-v2-fast Task 4) would cut the full-output redraw
  regardless of occlusion — but it's correctness-blocked (drag-shake) and, given cinnamon
  is already smooth, not urgent.

## Measured: cinnamon is smooth (no problem to fix)
Dual fullscreen (glxgears + YouTube, one per output), cinnamon:
`frame_present_count/s = 100–119`, `missed_pageflips/s = 0`, `vk_queue_wait_idle/s = 0`,
`cpu_fence_wait_ns/s = 0`. Zero dropped frames. The present/compose/flip path is sound.

## Where "choppy" actually came from
- **fvwm**: heavy X traffic / redraws saturate yserver's single-threaded loop ("many
  things slow on fvwm"). WM issue, not present-path. → use e27/cinnamon for perf testing
  ([[reference_fvwm_slow_use_e27]]).
- **e27**: redirects everything; some transient depth-32 gadget always on top; its own
  quirks. Smooth-ish but not the bug it first looked like.

## Branch disposition — BOTH DELETED 2026-07-08 (not feasible)
Both blocked by the same `participating=0` wall; neither shippable for a compositing
desktop. Deleted to avoid dead-branch cruft. Key learnings preserved here so nothing is
lost if either is ever revisited:

- **`feat/direct-scanout-m1`** (deleted) — M1 proved: the RX 580 **can** import a client
  DRI3 dma-buf as a scanout DRM framebuffer, but only via a modifier-less `add_fb2` that
  lets the **kernel infer tiling** — because DRI3 hands yserver `modifier=0` (LINEAR) for
  buffers that are actually tiled, so an explicit-modifier `add_fb2` returns EINVAL. To
  ever use this, yserver must recover the buffer's *real* modifier (DRI3 v1.2 or BO
  metadata), not trust the reported 0. The branch also added DRI3 modifier+pitch retention
  on `ImageBacking::Imported` and a per-output coverage predicate — re-implementable from
  the scope doc if #7 revives. Recipe `yserver-scanout-probe` (TEST_ONLY HW-safe probe)
  went with it.
- **`feat/occlusion-cull`** (deleted) — top-level occlusion cull (`fullscreen_occluder_index`,
  TDD'd) + `yserver-occlusion-test` recipe. Inert on compositing WMs (covering window is a
  COW-subtree child, not a top-level). Design in the scope doc
  `2026-07-08-occlusion-cull-fullscreen-scope.md` (on master) if COW-subtree occlusion is
  ever pursued.
- Shipped to master earlier this session (unrelated, real wins): #1 glyph-draw batching
  (`ae5f6bc7`), #2 A1-glyph cache (`c35ac33f`).

## Lesson
Establish "is it even slow under a *good* WM?" **before** scoping an optimization off a
symptom seen under one WM. The #7/occlusion effort was scoped off fvwm choppiness; a
single cinnamon run up front would have shown there was no present-path problem to fix.
