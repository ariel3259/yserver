# Clipped repaint, first measurement: per-compose GPU time roughly halves

**Status: step 4 works, and the saving is the pixel count. Magnitude sound,
exact percentage confounded. Absolute values do not transfer off this box.**

silence (RX 6800), 2560×1440, MATE **non-composited** (confirmed), a few
terminals and the same windowed mpv clip, with some window dragging. Branch
`fix/noncomposited-damage-repaint` at `cff35cba` against master `ad318afa` — the
commit the plan names as the no-regression reference — built and run from a
separate worktree so both used their own target directory.

Raw: `data/2026-09-02-step4-mate-silence-telemetry.log` (121 composing buckets)
and `data/2026-09-02-step4-mate-silence-master-baseline.log` (58).

## Result

| metric | master | branch | delta |
|---|---|---|---|
| `avg_gpu_render_ns` (median) | 197,367 | **87,741** | **−55.5%** |
| `avg_compose_cb_record_ns` | 115,875 | **96,452** | **−16.8%** |
| `damage_fraction` | 1.000 | 0.380 | −62% |
| descriptors per compose | 17.5 | **8.0** | −54% |
| implied GPU ms per wall second | 11.4 | **4.9** | −57% |

`damage_fraction = 1.000` on master is not a measurement, it is the always-Full
path by construction. On the branch it reports the area actually rasterised.

## Why this is more than a ratio of two medians

**Pixel throughput is unchanged.** Master moved 3.69M pixels in 197 µs
(18.7 Gpix/s); the branch moved 0.380 × 3.69M = 1.40M pixels in 88 µs
(16.0 Gpix/s). The GPU is doing the same work per pixel and simply being asked
for fewer — which is precisely the mechanism this project asserted, arrived at
from the other direction.

The 15% throughput shortfall is the fixed per-compose cost clipping cannot
remove: scissor setup, the LOAD, and every draw call and descriptor bind that
still gets recorded. It agrees with the 40-110 µs fixed floor the damage audit's
`fixed + k·area` fit found independently on different hardware.

**CPU fell.** 116 → 96 µs of command-buffer recording, with descriptors per
compose down from 17.5 to 8.0 — the draw cull more than paying for the region
algebra. The concern that this work would trade GPU for CPU does not survive
contact: both went down.

## Confounds, both directions

- **The workloads were not identical.** `paint_submits/s` was 424 on master
  against 1464 on the branch. More client painting means wider damage and less
  to clip, so this biases *against* the branch — the measured win is
  conservative on that axis.
- **Per-compose time falls with load** through clock ramping (~1.3× across this
  load range, from the z400 bucket analysis in
  `2026-09-02-yserver-gpu-share-cinnamon.md`). Master ran lighter, so part of its
  197 µs may be a downclocked GPU, which would *exaggerate* the win. This data
  cannot separate that.

What survives both: the two **clock-independent structural** measures — painted
fraction 0.380 and descriptors per compose −54% — land at the same ratio as the
timing. So "roughly halved" is safe; "55.5%" is not.

## Correctness

No panic and no assertion failure across 246 telemetry buckets with
`-C debug-assertions=yes`, so `painted ⊇ repaint` held, nothing staged twice,
culling never removed the opaque draw the clipped path depends on, and no BO was
left missing pixels it had just painted.

Nothing visually wrong through terminals, windowed mpv, dragging and menus.
Across awesome and MATE: **`no_opaque_cover = 0`** — the opaque-cover guard
found its covering draw on every clipped frame — and `unloadable_bo = 4` on
MATE, which is three BOs on first acquire plus one event, i.e. the gate firing
exactly when it should and not otherwise. Those are the two gates whose failure
mode is stale pixels.

`empty_draws = 0` and `copied_route = 0`; all 2277 other fallbacks were
`threshold`.

## What this does not show

- **The absolute share.** This is an RX 6800; the ratio transfers to the z400,
  the microseconds do not. The open amdgpu_top cross-check is unaffected.
- **Anything about drag.** 31.4% of frames rendered Full and essentially all of
  them were `threshold` — dragging still posts whole-output damage through
  `mark_scene_structure_dirty`, exactly as step 4 predicted. So this measurement
  understates what is reachable: step 2 is what makes those frames partial.
- **The per-BO model under stress.** The audit still keeps one candidate image
  per output and cannot test `missing[bo]` at all (4.7, unbuilt). The 300-step
  rotation unit test is what covers it today.

## Also confirmed here

Resizing a window shows black on **both** master and the branch, so it is
pre-existing and not caused by clipped repaint. It does not happen on Xorg,
which makes it ours — see `project_resize_black_window_storage` in the notes.
