# Test status — latest numbers

Snapshot of the current xts5 (X Test Suite) and rendercheck (RENDER
smoke) pass rates. This file is the headline only; run-by-run history
and debugging notes live in [`xts-baseline.md`](xts-baseline.md) and
`status.md`.

## xts5 — full run #6, yserver/KMS bare-metal (eiger/aarch64, 2026-06-25)

**3993 / 5987 test purposes PASS (66.7%)** — +32 vs run #5 (3961),
FAIL 818 → 813, UNRES 80 → 67. 

Movers: XIproto +8, Xlib4 +4, Xlib8 +4, Xlib9 +4, Xlib7 +3, Xproto +3, Xlib12 / Xlib13 / Xt13 +2 each.

For context, Xorg itself only passes 77% of the test suite.

| scenario  | cases | tests | PASS | FAIL | UNRES | UNTST | UNSUP | NOTIU | Δ PASS |
|-----------|------:|------:|-----:|-----:|------:|------:|------:|------:|-------:|
| Xproto    |   122 |   389 |  361 |    6 |     1 |    19 |     2 |     0 |     +3 |
| Xlib3     |   109 |   162 |  107 |   18 |     3 |    27 |     6 |     1 |      0 |
| Xlib4     |    29 |   324 |  171 |  113 |     7 |    17 |    11 |     5 |     +4 |
| Xlib5     |    15 |    84 |   59 |   18 |     0 |     5 |     2 |     0 |      0 |
| Xlib6     |     8 |    50 |    6 |   15 |     0 |    29 |     0 |     0 |      0 |
| Xlib7     |    58 |   172 |   86 |   28 |     0 |    13 |    45 |     0 |     +3 |
| Xlib8     |    29 |   165 |   92 |   37 |     4 |    22 |    10 |     0 |     +4 |
| Xlib9     |    46 |  1472 |  833 |  376 |     0 |    36 |    23 |   201 |     +4 |
| Xlib10    |    23 |    95 |   25 |   29 |     5 |    35 |     1 |     0 |      0 |
| Xlib11    |    33 |   195 |   74 |   49 |     3 |     4 |    22 |    43 |      0 |
| Xlib12    |    27 |   138 |   96 |   12 |     1 |    15 |     2 |    12 |     +2 |
| Xlib13    |    32 |   269 |  207 |   23 |    23 |    10 |     3 |     3 |     +2 |
| Xlib14    |    45 |    58 |   46 |    7 |     0 |     5 |     0 |     0 |      0 |
| Xlib15    |    45 |   159 |  125 |    1 |     0 |    33 |     0 |     0 |      0 |
| Xlib16    |    30 |   105 |   82 |    0 |     0 |    22 |     1 |     0 |      0 |
| Xlib17    |    55 |   131 |  102 |    8 |     0 |    21 |     0 |     0 |      0 |
| Xopen     |     8 |   127 |  122 |    3 |     0 |     0 |     2 |     0 |      0 |
| Xt3       |    21 |    73 |   73 |    0 |     0 |     0 |     0 |     0 |      0 |
| Xt4       |    33 |   192 |   94 |    0 |     0 |    98 |     0 |     0 |      0 |
| Xt5       |    10 |    69 |   26 |    0 |     0 |    41 |     0 |     0 |      0 |
| Xt6       |     7 |    71 |   67 |    4 |     0 |     0 |     0 |     0 |      0 |
| Xt7       |    11 |   106 |   96 |    1 |     0 |     6 |     0 |     3 |      0 |
| Xt8       |     7 |    43 |   35 |    4 |     0 |     4 |     0 |     0 |      0 |
| Xt9       |    33 |   189 |  122 |    2 |     8 |    55 |     2 |     0 |      0 |
| Xt10      |     8 |    17 |   16 |    0 |     0 |     1 |     0 |     0 |      0 |
| Xt11      |    58 |   285 |  246 |    3 |     0 |    34 |     0 |     0 |     −1 |
| Xt12      |    22 |    67 |   55 |    0 |     1 |    11 |     0 |     0 |      0 |
| Xt13      |    39 |   178 |  126 |    5 |     0 |    47 |     0 |     0 |     +2 |
| Xt14      |     2 |    18 |   18 |    0 |     0 |     0 |     0 |     0 |      0 |
| Xt15      |     1 |     2 |    0 |    0 |     0 |     0 |     2 |     0 |      0 |
| XtC       |    29 |   147 |   88 |    1 |     1 |    56 |     1 |     0 |      0 |
| XtE       |     1 |     1 |    1 |    0 |     0 |     0 |     0 |     0 |      0 |
| ShapeExt  |    11 |    11 |   11 |    0 |     0 |     0 |     0 |     0 |      0 |
| XI        |    36 |   316 |  222 |   49 |    10 |    28 |     2 |     5 |     +1 |
| XIproto   |    35 |   107 |  103 |    1 |     0 |     3 |     0 |     0 |     +8 |
| **total** | **1078** | **5987** | **3993** | **813** | **67** | **697** | **137** | **273** | **+32** |

ShapeExt, Xlib16 and Xt3/4/5/10/14 are fully clean (zero
FAIL/UNRES). yserver survived the whole sweep with zero panics in
the server log; 2 NORESULTs (`Xt5/XtUnmanageChild(ren)`, unchanged).

Movement vs #5 came from regular bugfixing, no focused xts work:
XIproto +8 (UNTST → PASS), Xlib4/8/9 +4 each, Xlib7/Xproto +3. The
minor new FAILs (XtC +1, Xt11 −1 PASS fontset/resource, XIproto
ChangeFeedbackControl) are unrelated to the e27 input fixes — those
touched XI2 delivery, which xts5 doesn't exercise.

Largest FAIL buckets / next targets:
1. **Xlib9 (376)** — remaining drawing/GetImage content semantics.
2. **Xlib4 (113)** — depth-mismatch BadMatch (CWBorderPixmap parser
   needed), colormap visual-type checks, bit-gravity pixel cluster,
   stacking-order pixel checks, BadAccess event-mask conflicts.
3. **Xlib11 (49)** — residual grab semantics.
4. **XI (49)** — XInput-1.x device functions + XTest-through-XI1 gaps.
5. **Xlib8 (37)** / **Xlib7 (28)** — events / colormap sections.

Previous full runs:
- #5 — 2026-06-07 22:03:01 (bee, HW): 3961/5987 PASS (66.2%) —
  `xts/results/2026-06-07-22:03:01/`.
- #4 — 2026-06-07 17:14:17 (bee, HW): 3747/5987 PASS (62.6%) —
  `xts/results/2026-06-07-17:14:17/`. Last run before the Xlib4
  BadX work and the desktop-input-fixes branch.
- #3 — 2026-06-06 (air, M1): 3419/5987 PASS (57.1%) —
  `xts/results/2026-06-06-20:26:54/`.
- #2 — 2026-06-05 (M2) + 2026-06-06 air XI row: 3370/5987 PASS (56.3%)
  — `xts/results/2026-06-05-13:20:07/` (+ `2026-06-06-00:58:03` for XI).
- #1 — 2026-06-04 (first ever to complete): 2784/5987 PASS (46.5%) —
  `xts/results/2026-06-04-15:48:44/`.

Aborted run between #3 and #4 (`xts/results/2026-06-07-14:01:34/`):
2999/5987 PASS, 1290 UNRES — the GetImage BadMatch cascade caused
by an unguarded `XConfigureWindow` on the root window, fixed by
`77f785b` before run #4.

## rendercheck — bare-metal 2026-06-04, rendercheck 1.6, 900 s/test

| category    |  PASS | TOTAL |
|-------------|------:|------:|
| fill        |    64 |    64 |
| dcoords     |     2 |     2 |
| scoords     |     1 |     1 |
| mcoords     |     1 |     1 |
| tscoords    |     2 |     2 |
| tmcoords    |     2 |     2 |
| blend       |     5 |     5 |
| composite   |     5 |     5 |
| cacomposite |     5 |     5 |
| gradients   |  6081 |  6081 |
| repeat      |   380 |   380 |
| triangles   |   570 |   570 |
| bug7366     |     1 |     1 |
| **total**   | **7119** | **7119** |

**100% pass.**

> Use rendercheck ≥ 1.6. Version 1.5 has a bug in
> `gradients::render_to_gradient_test` that trips even against the
> host X server.
