# Damage-completeness audit — interim finding

Phase 0 of
`docs/superpowers/specs/2026-09-01-noncomposited-damage-repaint-design.md`,
implemented per
`docs/superpowers/plans/2026-09-01-damage-completeness-audit-plan.md`.

**Status: no damage-completeness hole found across ~9800 partial comparisons on
two machines and two window managers.** The single divergence ever observed
resolved to a boot-path ordering bug — a real defect on master, now fixed and
verified — not a damage hole.

That makes this a **clean baseline for the regression test**, which is what the
audit is for under the superseding design: any mismatch appearing after derived
damage lands is a regression introduced by that work, not pre-existing.

Still not a formal P2 qualification: no long soak, and coverage is MATE and
Window Maker only. Under the superseding design that no longer gates anything. The instrument is validated. Steady-
state paint damage measured clean over 8999 partial comparisons, but a startup
hole is confirmed, so damage is known incomplete.

Raw data: `data/2026-09-01-damage-audit-mate-run.log` (silence),
`data/2026-09-01-damage-audit-bee-seed1.log` (bee).

Hardware: **silence** — RX 6800, dual 2560×1440, release `b0cf410f`, seed frame
60 for the paint runs and frame 1 for the startup runs, non-composited MATE.
**bee** — 6900HX APU, single 2560×1440, seed frame 1, non-composited MATE and
Window Maker.

Cumulative clean partial comparisons: ~8999 (silence, MATE, scrolling terminal)
+ 465 (bee, MATE) + 297 (bee, Window Maker, including a runtime root refill).

## The instrument works — validated, not assumed

An injected hole was detected. `unmap_subwindow` was changed to call
`wake_for_damage()` instead of `mark_scene_structure_dirty()` — keeping the
scheduling, dropping only the damage rect. Closing a menu produced:

```
frame=1281  tile=64  pixel=0,27  candidate=0xffaeaeae  reference=0xff313131
            ledger=…backend.rs:17282:missing (+18x 15034)
```

`0xffaeaeae` is the menu grey left stale in the candidate; `0xff313131` is the
desktop that should be behind it. Correct signature, correct site, marked
`:missing`. Without this the clean results below would be worthless.

**Attribution is a candidate set, not a name.** That mismatch listed 19 events:
the injected one plus 18 `mark_dirty()` calls. `mark_dirty()` claims a
full-output `expected_area`, so every one of them is a candidate for every
tile. The conclusion "a region-less wake failed to report" holds; "which call"
does not. Narrowing needs real expected areas on specific sites, which the plan
stages for after a suspect emerges.

## Results

| output | comparisons | idle | partial | full | mean_damage | mismatches | resets |
|---|---|---|---|---|---|---|---|
| 0 | 16115 | 7004 | **8999** | 112 | 0.078 | 0 | 0 |
| 1 | 23676 | 23550 | 0 | 126 | 1.000 | 0 | 0 |

Workload: MATE idle, then a terminal running `yes` on output 0. Output 1 had no
paint workload, hence `partial=0`.

No `run_suspect`, no ledger-bound warnings, `interval=1` throughout — so nothing
disqualifies the run under the plan's criteria.

**8999 partial comparisons at 7.8% mean repaint area, all clean.** For a
scrolling terminal, `projected_damage` covered every pixel whose composited
value changed, nine thousand times running. That is the first non-vacuous
evidence in this campaign that the paint path's damage is complete.

`mean_damage=0.078` is also the first measured estimate of the prize: clipped
repaint would rasterise ~8% of the output on this workload.

## Measured: what clipped repaint actually saves

The audit composes the same scene twice per frame — candidate clipped to the
damage bbox, reference full — so timing both is a direct A/B on identical
input. bee, single 2560×1440. Raw data: `data/2026-09-01-damage-audit-bee-*-timed.log`.

| scene | full | clipped | mean_damage | saving | gpu_n |
|---|---|---|---|---|---|
| Window Maker + wezterm | 443 µs | 233 µs | 0.370 | 210 µs (47%) | 548 |
| MATE desktop | 521 µs | 250 µs | 0.433 | 271 µs (52%) | 1178 |

**These numbers are not representative of the hardware this work exists for.**
See §The motivating case below before drawing any conclusion from them: on a
modifier-less Polaris card at 4K a compose costs 6.8 ms, not 0.5 ms, and the
addressable fraction is far larger.

On bee, a compose costs about half a millisecond and clipping halves it —
roughly 1.5% of a 60 Hz frame. Fitting each pair as `fixed + k·area` gives a fixed
floor of ~40-110 µs that clipping can never remove, because a clipped pass still
records every draw call, descriptor bind and pipeline switch; only fragment work
shrinks.

Two things push the real number up. Fragment cost scales with pixels, so 4K is
~2.25× this share, and multi-output multiplies it; at 144 Hz the same 0.5 ms is
6.4% of budget rather than 2.7%. And GPU time is not the only benefit — less
work is less power, which on a laptop with a mostly-static screen may matter
more than frame time does.

**Today's ratio understates the full stack.** MATE's split is `partial=732,
full=447`: those 447 whole-output frames are window management, which is
full-output *by construction* under today's `mark_scene_structure_dirty`. They
are in the mean and they save nothing. Derived damage step 2 is what makes them
tight. On MATE's fit, pulling the mean from 0.43 to ~0.18 would put clipped
compose near 130 µs against 521 µs — a ~390 µs saving. So step 4 measured against
today's damage is not step 4's ceiling; step 2 is the multiplier.

### The motivating case: RX 570 at 4K, 6.8 ms per compose

From discussion #56 (BergmannAtmet), which is the workload this project exists
to fix:

- RX 570 (Polaris), **3840×2160**, mpv windowed at ~960×540 playing 1080p50.
- His log reports `physical device lacks VK_EXT_image_drm_format_modifier`, so
  scanout buffers are **LINEAR** — far slower to render into than tiled.
- yserver composites the whole 4K output ~60×/s at **6.8 ms of GPU time per
  compose, ~40% of the 16.7 ms frame budget.** Xorg non-composited does
  essentially none of this work, because it does not recomposite at all.

6.8 ms against bee's 0.5 ms is 13.6×; area explains 2.25× and the rest is
Polaris plus LINEAR. The bee measurements above are a fast GPU at 1440p with
tiled buffers — the wrong box, resolution and buffer layout for this question.

**The window is 6% of the screen area.** Applying bee's fixed-plus-fragment
split (~10% fixed, ~90% area-proportional), a tight-damage compose there would
be on the order of **1 ms** — compose falling from ~40% of frame budget to ~6%.
That is the size of the actual prize, and it is large.

**`damage_fraction=1.000` in the telemetry is not evidence of full-screen
damage.** On the `Repaint::Full` path `tick_one_output` calls
`record_damage_pixels(full_area, full_area)`, so it reports 1.000 by
construction on every frame regardless of the real damage set. The audit's
`mean_damage`, computed from `output_damage.bounding_rect()`, is the only
measurement of actual damage extent this project has produced.

**Still to measure**, and it is the number that sizes the win: `mean_damage`
on that class of hardware with mpv windowed and the terminal quiet
(`--msg-level=all=fatal`, since mpv's status line makes mate-terminal copy a
3836×2090 pixmap ~50×/s, a genuine near-full-screen damage source).

### Measured on target hardware: production compose is 3-6.3 ms

z400 + RX 460 (Polaris), 2560×1440, non-composited MATE, plain telemetry run
with the audit off. `avg_gpu_render_ns`: 2.97, 4.64, 6.33, 5.92, 4.00, 3.76 ms.
At the observed ~44 fps (`full_redraw_fallback/s=43-45`) that is roughly 18-28%
GPU spent on compose alone, which accounts for the reported load and
corroborates the 6.8 ms measured on an RX 570 at 4K.

**The control that matters: Xorg runs the same workload on the same hardware,
into the same LINEAR scanout buffer, for almost no GPU cost.** (For the
*engineering* target use a wlroots compositor rather than Xorg — same rendering
model, same LINEAR constraint on this card. Xorg does no compositing pass at
all, so its number is a lower bound our architecture cannot reach. See the
spec's acceptance section.) The cost is
therefore not intrinsic to the hardware or the buffer layout. The difference is
how many pixels get written per frame: Xorg writes the damaged region, yserver
writes the whole output, every frame, unconditionally.

A note on attribution, because this has been got wrong before. The audit
measured 0.56 ms for the same scene, but the audit composes into a private
`OPTIMAL`-tiled image while production composes into the LINEAR scanout BO, so
its **absolute values do not transfer to this hardware — only the ratio does.**
It is tempting to read the 0.56 ms vs 4 ms gap as "LINEAR is the problem". It is
not: LINEAR raises the price per written pixel, and the Xorg control proves that
price is affordable when you write few enough of them. The fix is writing fewer
pixels, which is clipping, and it works irrespective of layout. See
[[feedback_dont_revive_excluded_hypotheses]] — the linear/modifier theory has
been a recurring wrong turn on this codebase.

Since the dominant cost is proportional to pixels written, clipping scales it
down directly: at `mean_damage=0.356` a clipped compose should land near 1.5 ms
against 4 ms, saving on the order of 15 points of GPU utilisation.

### The target number: labwc does this for 2.8% GPU

`amdgpu_top` on the z400 (RX 460, Polaris11/GFX8), **labwc** — a wlroots
compositor — playing windowed mpv. 20 samples, raw data in
`data/2026-09-01-labwc-baseline-rx460.json`:

| process | GFX mean | GFX max | CPU mean |
|---|---|---|---|
| mpv | 3.9% | 4% | 39.8% |
| **labwc** | **2.8%** | 5% | 3.6% |

**This is the acceptance baseline.** Same card, same workload, same rendering
model (scene graph, per-surface buffers, compositing pass per frame). yserver's
compose alone costs 3-6.3 ms at ~44 fps — 13-28% GPU — so we are **5-10× labwc**.

The arithmetic reaches the target: mpv windowed covers a few percent of the
screen, and if compose scales with pixels written, 4 ms at ~6% area is ~0.3 ms,
which at 44 fps is ~2% GPU. Parity.

Caveat, stated without leaning on it: wlroots uses GBM/GLES and may be obtaining
tiled buffers where yserver's Vulkan reports no
`VK_EXT_image_drm_format_modifier`, so buffer layout may not be held constant by
this comparison. Worth checking eventually. It does not change the lever —
pixels written is what we control and what the 3-6 ms is attached to.

### Clipping must have a damage-fraction threshold

An early Window Maker sample at `mean_damage=0.857` measured `clipped_us=208.7`
against `full_us=199.3` — **clipping cost more than full**. Small sample (n=41),
but the direction is real: a sub-rect `CLEAR` plus scissor setup does not pay
for itself at high damage fractions, particularly with delta colour compression.

Above roughly 60-70% damage, render Full. Without that threshold clipped repaint
is a net loss on exactly the frames that are whole-output today.

## Only `idle` and `partial` comparisons are evidence

A comparison on a frame whose damage covers the whole output is a tautology —
the candidate was wholly recomposed, so it cannot fail. Window management
damages the whole output by construction (`mark_scene_structure_dirty`), so
drag, resize, restack, menu open/close and moving a window over another produce
`full` comparisons almost exclusively.

An earlier run in this session recorded 5556 clean comparisons across exactly
those interactions and proved nothing. The `idle`/`partial`/`full` split was
added because of it. **Any future run reporting a clean result must cite its
`partial` count, or it is not evidence.**

This is not a defect in the audit. It restates the spec's corollary: structural
cases are safe today precisely because they are full-output, and they are
therefore also cases Phase 2 cannot speed up.

## One hypothesis refuted

**"A scene draw appearing with no damage."** The first two runs showed a
whole-screen divergence at frame 2 — every tile, both outputs, candidate
`0xff000000` (the `bg` clear), reference `0xff505050`. The reading was that the
draw list went empty → non-empty with no damage covering it, which would have
implicated invariant 3 (weakened for Phase 2 during spec review). Adding draw
counts to the log killed it: `seed_draws=1, draws=1`. Nothing structural
changed; the same single draw rendered black in the candidate and grey in the
reference.

## RESOLVED: frame 2 is a boot-path damage/paint ordering inversion

**Not a damage-completeness hole, and not a sampling artefact.** Both earlier
readings in this document were wrong; the mechanism below is established from
matching constants and named code paths, not inference.

The `sampled` diagnostic named the drawable: `sampled=[(1, 1)]` at both the seed
frame and the mismatch — DrawableId 1, xid 1, which is the **root window**
(`core.window_id = 1`, kms/core.rs:1986; allocated `DrawableKind::Root` at
backend.rs:4761). And `reference=0xff505050` is exactly yserver's own root init
fill constant, `self.core.bg_pixel.unwrap_or(0x0050_5050)` (backend.rs:~4781).

The chain:

1. `init_root_storage` issues `engine.fill_rect(… 0x505050 …)` — a **buffered**
   paint in the submit group.
2. `composite_and_flip` (lib.rs:396) calls `scene.tick()` **without flushing
   it.** The other `scene.tick()` caller, `maybe_composite`, does flush, with
   the comment "so scene.tick observes all paint CBs already submitted"
   (backend.rs:15243). The boot path skips it.
3. Frame 1 therefore composes root storage *before the fill executes* → black.
   Candidate and reference agree because both read the same pre-fill storage —
   which is exactly why `at_seed=false`.
4. Frame 1's compose peeks the root's presentation damage and acks it at retire.
5. By frame 2 the fill has landed via the first `maybe_composite` flush, so the
   reference composes grey, with no damage left to update the candidate. It
   latches until full-output damage arrives at frame 91.

Deterministic across two machines because it is an ordering bug, not a race.

**This is a real bug on master.** The first flip after boot can present pre-fill
content — a brief black flash instead of the root background. Always-Full hides
it from frame 2 onward. Fix: give `composite_and_flip` the same
`close_open_frame` + `flush_submit_group` that `maybe_composite` does.

**Fix verified by the instrument that found it.** `1504230b` (also cherry-picked
onto the audit branch as `9452ae5c`) adds the flush. Re-run on bee, same seed
frame and recipe: **0 mismatches, `episodes_opened=0`**, against 4096 and 4096
before. `resets=0` both runs.

**Still unsettled: does `fill_rect` record presentation damage?** The verifying
run does *not* answer this, contrary to what was expected of it — with the fix
the root is already filled at frame 1, so nothing changes between frames 1 and 2
and there is no content change for damage to report either way. Divergence would
have implied a hole; the absence of divergence implies nothing.

**Answered: root fills do reach `output_damage`.** Tested under **Window
Maker**, which paints the root itself and puts no desktop window over it, so the
root is genuinely visible — unlike MATE, where caja's desktop window occludes it
and the test would have come back clean for the wrong reason. `xsetroot -solid`
during a running session: the background visibly changed and the audit reported
**0 mismatches** across 297 partial comparisons (`mean_damage=0.394`,
`resets=0`). Raw data: `data/2026-09-01-damage-audit-bee-wmaker.log`.

The inference holds because the candidate is only updated within
`output_damage`: had the fill reported nothing, the candidate would have kept
the old background while the reference changed, and with `interval=1` plus a
1s idle re-compare there was no window for that to slip through.

### Superseded reading: "a real damage hole" (kept for the record)

The seed-time comparison (`f05f210e`) settles it. On **bee** (6900HX APU,
single 2560×1440, non-composited MATE, seed frame 1), raw data in
`data/2026-09-01-damage-audit-bee-seed1.log`:

- **`at_seed=false` on all 4096 mismatches.** The seed-frame comparison was
  clean: two independent full composes of the same scene at the same instant
  agreed. The compose is deterministic and does not read storage before a paint
  lands.
- The divergence is at **frame 2**, whole screen, `candidate=0xff000000` (the
  `bg` clear) vs `reference=0xff505050`, with `seed_draws=1 draws=1`.
- Attribution: 12 candidate events, **all** `backend.rs:15034:missing` —
  `KmsBackend::mark_dirty()`.
- `resets=0`, `interval=1`, so nothing disqualifies the run.
- `comparisons=1079`, `idle=853`, `partial=114`, `full=112`.

Identical in every respect to silence (RX 6800, dual 2560×1440) except event
count — 15 there, 12 here. Different GPU, different memory architecture,
different output count, same result. A timing artefact would not survive that.

**Conclusion.** Between frame 1 and frame 2 the composited result changed and
`output_damage` did not describe the change. The draw list is unchanged
(`draws=1` both frames), so it is the *content* of one drawable that changed
without reporting damage. That falsifies the assumption stated in
`mark_dirty()`'s own comment — "Paint paths already record per-drawable
presentation damage" — at least once.

**How much it means.** It is a startup hole: it healed and never recurred
across ~1079 frames, and today's always-Full rendering hides it completely, so
there is no user-visible bug. Its value is as proof that the assumption is
false, and as the first concrete entry on the work list for derived damage.

**Next diagnostic.** `composite_and_flip` runs before any client connects
(lib.rs:396), so the single draw at seed is almost certainly root storage and
the grey is MATE painting the desktop. Logging the draw's source `DrawableId` /
xid at seed and at first mismatch turns "a paint did not report" into "*this*
paint did not report". One log line, one boot.

### Superseded reading (kept for the record)

Seeding at frame 60 instead of frame 1 eliminated the divergence entirely.
**That is not exoneration.** It shows the event happens before frame 60; a late
seed simply no longer straddles it. An earlier revision of this document called
it a startup sampling artefact on that basis, which was wrong.

Two readings still fit every observation:

- **A real hole.** Something changed the screen between frames 1 and 2 and
  produced no damage — note the candidate then stayed wrong for **89 frames**,
  healing only at frame 91 when full-output damage arrived. A paint that
  produced damage would have healed at frame 3.
- **A sampling artefact.** The seed's compose read drawable storage whose paint
  had not landed, so nothing ever actually changed on screen.

The flush-ordering explanation was checked from source and **ruled out**:
`maybe_composite` flushes the submit group with `FlushReason::SceneCompose`
before `scene.tick()`, precisely "so scene.tick observes all paint CBs already
submitted" (backend.rs:15243). The seed does run under the *other*
`scene.tick()` caller — `composite_and_flip` (backend.rs:8590), which performs
no flush — but that path runs once at startup **before any client connects**
(lib.rs:396), so there are no buffered client paints for it to miss.

### The discriminator, now implemented

`f05f210e` composes the reference on the seed frame and compares immediately.
Both images are then full composes of the same scene at the same instant, so:

- **mismatch at `at_seed=true`** ⇒ the two composes disagree ⇒ sampling
  artefact, and the audit needs a fence/flush before the seed.
- **clean at seed, mismatch at frame 2** ⇒ content genuinely changed with no
  damage covering it ⇒ a real hole, and a Phase 2 blocker, because a canonical
  image would latch it exactly as the candidate did for 89 frames.

Run with `YSERVER_DAMAGE_AUDIT_SEED_FRAME=1` (now the recipe default).

**Whichever way it resolves, a late seed must not be the answer.** A late-seeded
audit is blind to every startup hole by construction, and startup is where the
one candidate hole found so far lives.

## Remaining work before P2 can be qualified

- A long soak with a paint workload running (a scrolling terminal or looping
  video), so it accrues `partial` comparisons unattended rather than only
  `idle` ones.
- A second paint workload with a different damage shape — mpv — and on the
  second output, which so far has only ever been idle.
- awesome, i3 with a floating drag, and a non-MATE non-composited desktop.
- Settle the peek sub-question above (one log line).
- P2 is **not disqualified, and not qualified either**: the only divergence
  found turned out to be an ordering bug, and steady-state paint damage
  measured clean over 8999 partial comparisons. Coverage is still one WM, one
  workload shape, and no long soak. Under the superseding design
  (`../specs/2026-09-01-damage-derived-scene-repaint-design.md`) that changes
  the remit: damage becomes complete by construction, and this audit becomes
  the regression test that proves it.

## Cost note

Each comparison costs two extra full composes, two full image→buffer copies
(~29 MB at 2560×1440), a compute dispatch and three blocking fence waits inline
in the render tick. It is audible on the fans. The available saving is to bind
the audit images directly as `readonly image2D` — they already carry `STORAGE`
usage — and drop the buffer copies. Do that before any multi-hour soak.

Do **not** raise `YSERVER_DAMAGE_AUDIT_INTERVAL` to reduce load: a divergence
can heal between samples, so `interval > 1` disqualifies a run.
`YSERVER_DAMAGE_AUDIT_IDLE_SECS` is safe to raise but only affects idle
sampling.
