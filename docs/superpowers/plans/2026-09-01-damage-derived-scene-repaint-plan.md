# Stop repainting the whole screen — implementation plan

> **Status 2026-09-03 — campaign closed by decision (jos).** Steps 0-4 and step 1
> (stages A/B/C, restack fix) are on `fix/noncomposited-damage-repaint`, smoked
> on silence, bee and the z400, and confirmed by the #131 reporter (nvtop
> 12%/44% → 4%/22%, Xorg 3%/10%). Measured share on the target: yserver 4.27%
> GFX vs labwc 2.80%. Stopped here because non-composited desktops are the edge
> case; most users run composited. Recorded but **not** pursued: blend-off on the
> opaque pipeline (free), the LINEAR-vs-OPTIMAL audit A/B on modifier-less GFX8
> (measure, never argue), the clip-threshold retune on the RX 460, and the
> tick-driver walk-skip for e16's CPU cost. Findings:
> `../findings/2026-09-03-step1-z400-first-measurement.md`,
> `../findings/2026-09-03-naive-occlusion-cull-postmortem.md`.

Implements `../specs/2026-09-01-damage-derived-scene-repaint-design.md`. Read
that spec first; this plan does not restate its reasoning, only what to build,
in what order, and what proves each part.

Reviewed by codex 2026-09-01/02, six rounds. The first two departures from the
design came back sound-with-a-caveat and the caveats are folded in below; the
third — replacing the design's mutation-time derived damage with a tick-time
scene diff — was forced by round 5 and is argued in step 2. The other findings
folded in: a stricter opaque-cover guard, the ordering of the `pending` feed
against the empty-projection force-compose, `painted` rather than the requested
repaint region at retirement, the pre-cull/post-cull draw-list invariant, one
latent descriptor-allocation bug promoted to a prerequisite, and the
safe-fallback helper of 3.4 covering the lifecycle paths the model cannot reason
about precisely.

**Branch:** `fix/noncomposited-damage-repaint` — the design, the audit, the two
fixes and now the implementation. One branch per campaign; an earlier draft of
this plan called for a separate `feat/` branch off master, which buys nothing
here and splits the context.

> **⚠ The size of the prize is probable, not proven — read
> `../findings/2026-09-02-yserver-gpu-share-cinnamon.md` before quoting a
> number.** The "compose alone is 13-28% GPU" premise came from
> `avg_gpu_render_ns`, our own timestamp bracket, while labwc's 2.8% came from
> amdgpu_top. On the first workload where amdgpu_top was pointed at yserver it
> reports **1.88%**, and physics agrees with amdgpu_top. That run was
> *composited* so it does not transplant onto the non-composited target, but the
> instrument mismatch is real and every document in this campaign inherits it.
> **Update, same day:** the MATE telemetry argues the premise back. Per-compose
> time *falls* with engine load (corr −0.50), which is the wrong sign for
> contention pollution, and every loaded bin implies 15-21% GPU against labwc's
> 2.80% total. So the prize is **probable, not proven** — one cross-check against
> amdgpu_top on a non-composited run would prove it, and that run does not need
> the z400.

**The deliverable is step 4.** Steps 0 and 3 exist to make it correct; steps 1
and 2 extend its reach afterwards. Judge every decision against GPU load on the
z400 + RX 460 versus labwc, not against internal tidiness.

## Prerequisites

- [ ] Merge `fix/boot-composite-flush` (1 commit off master; `composite_and_flip`
  composed without flushing buffered paint). It is a correctness fix on the
  compose path and clipped repaint makes stale-content bugs harder to read.
  Needs its eyes-on smoke first.
- [ ] Cherry-pick the `RegionSet::subtract` multiset fix from
  `fix/noncomposited-damage-repaint` onto the implementation branch. It is a real
  bug and is not on master.
- [ ] Keep the damage audit (`YSERVER_DAMAGE_AUDIT=1`) available on the
  implementation branch. It is the regression test for damage **producers**
  (steps 1 and 2). It tests nothing about steps 3 and 4 as it stands — see 4.7,
  which extends it into one.
- [ ] **Fix descriptor-allocation truncation** (scene.rs:5437-5453). On
  allocation failure `record_and_submit_render` `break`s, `record_command_buffer`
  then draws only `.take(descriptors.len())` (scene.rs:5662), and the caller still
  pushes a `PendingAck` and acks every snapshot as if the frame were complete
  (scene.rs:3800-3815, 1717-1719). Under always-Full that submits one visibly
  wrong frame and the next frame repairs it. Under clipped repaint the frame is
  recorded as painted, `missing[X]` is cleared for a region that was never drawn,
  and the hole is baked into that BO permanently. Make allocation failure abort
  the compose (the same shape as the other submit failures: fold repaint forward,
  no ack, no generation advance). `compose_submit_was_complete` (scene.rs:2924)
  already exists for the audit and is the predicate to reuse.

## The order decision, made without the deciding measurement

The design left one question open: whether steps 3 and 4 can precede steps 1 and
2, to be settled by measuring `mean_damage` for windowed mpv on the z400. That
run is not going to happen before implementation starts, so it is decided here
from the code and from the measurements already taken.

**Decision: 0 → 3 → 4 → 2 → 1.**

(The design's order was 1 → 2 → 3 → 4. Step 4 leads because it is the only step
that cuts GPU load and everything before it is bookkeeping the always-Full path
ignores. Step 2 follows because, as a tick-time diff, it no longer depends on
step 1 — it can use dst rects as a conservative stand-in for visible regions —
and it is what removes the whole-output hammer from drag, resize and restack, the
frames that are `full` by construction today. Step 1 lands last as a tightening
and correctness pass: occlusion culling, the parent-shape bug, and tighter
content damage.)

Evidence:

1. **Content damage is already tight, and it is measured.** The audit's MATE
   output 0 run — a scrolling terminal, i.e. one windowed client painting, the
   same shape as windowed mpv — recorded `mean_damage = 0.078` over **8999
   partial comparisons with zero mismatches**
   (`../findings/2026-09-01-damage-completeness-audit.md`). Projected content
   damage covers what changed and covers roughly 8% of the output.
2. **Nothing forces whole-output damage on that path.** `output_damage` is
   `projected_damage ∪ cursor damage ∪ scene_structure_damage ∪
   pending_repaint_after_failed_submit` (scene.rs:3444-3461). The ~22
   `wake_for_damage()` sites add **no damage** — they only set
   `scene_structure_dirty`, a wake flag (scene.rs:1183). Only the ~20
   `mark_scene_structure_dirty()` sites add whole-output rects, and every one of
   them is an event-driven mutator: create/destroy/map/unmap/configure/reparent/
   register/restack/shape/background/redirect/COW, plus two error paths. None
   fires per frame on a static desktop with one video window.
3. **The threshold makes the idle/second-output case a no-op, not a regression.**
   Audit output 1 (no paint workload) shows `mean_damage = 1.000`; those frames
   take the Full path under the damage-fraction threshold and cost exactly what
   they cost today.

**Falsifier, and what to do if it fires.** The first hardware run of step 4 must
report the audit's `mean_damage` well below 1.0 for windowed mpv — that is the
*producer* question this decision turns on (4.7) — and, separately, telemetry's
`damage_fraction` well below 1.0 with `full_redraw_fallback/s` well below
`frame_present_count/s`, which is the *cost* question. Both, because they can
disagree: tight producer damage that production still repaints in full means the
gates of 4.2 are rejecting the clipped path, which is a different bug with a
different fix. If it does not, something forces whole-output
damage that this reading missed: find which `mark_scene_structure_dirty` site
fires per frame (they are `#[track_caller]` and already feed the audit ledger),
and step 2 becomes the lead instead. Do not proceed to steps 1 and 2 on the
assumption that step 4 under-delivered for want of them without that number.

## Second departure: step 4 does not need step 1

The design derives the background as `damage − ⋃(opaque(n) ∩ visible(n))`,
because wlroots has no bottom layer — its background is a clear, so it must know
what covers it. **yserver already draws an opaque bottom layer.** `build_scene`
emits the root drawable as `draws[0]`, covering the whole output, with
`alpha_passthrough = false` (scene.rs:4094-4121); that variant forces `src.a = 1`
in the fragment shader against `ONE / ONE_MINUS_SRC_ALPHA` blending
(vk/pipeline.rs:326-341), so it fully overwrites the destination.

So for any region covered by an opaque draw, `loadOp = LOAD` + scissor +
redrawing the draws that intersect the region reproduces exactly what a full
compose produces there. No background algebra, no visible regions, no
occlusion — the `CLEAR` a full compose performs is invisible under the root
anyway.

That turns step 1 from a prerequisite into an optimisation (cull occluded draws,
tighten projected content damage) plus the fix for the parent-bounding-shape bug,
and it lets the GPU win land far sooner. **The correctness obligation moves into
a guard** (step 4, task 4.2): take the clipped path only when the repaint region
is contained in the destination rect of an opaque draw. Otherwise render Full.

## Step 0 — a real region type — **DONE** (`region.rs`, 2026-09-02)

Landed unused: `#[cfg_attr(not(test), expect(dead_code, …))]`, so the annotation
itself starts warning the moment step 3 wires it up. 19 tests, including a
400-case randomised differential test of union/subtract/intersect/area against a
brute-force pixel oracle, plus direct assertions on the three canonical
invariants — a region can cover the right pixels while being non-canonical, and
the vertical-merge comparison silently degrades if it is.

Implementation note: set ops decompose on y and combine 1-D x-spans per slice,
rather than merging bands incrementally as pixman does. `O(n·m)` against
`O(n+m)`, which is the right trade with a 32-rect cap and damage regions of a
handful of boxes — 1-D interval algebra is far easier to get right, and the
consequence of getting it wrong here is indistinguishable from a damage bug on
screen.

`RegionSet` (store.rs:499) is a `Vec<Rect2D>` whose `subtract` removes **exact
rect matches only**, as a multiset. That is deliberate and correct for the
snapshot/ack path, where an identical damage rect can arrive twice while the
first snapshot is in flight — geometric subtraction would drop the newer one
(the `355c221f` lesson). It is unusable for step 3: `missing[X].subtract(repaint)`
against a bbox would match nothing, `missing[X]` would grow to the 256-rect cap,
collapse to extents, and pin every frame to Full forever. Silent, safe, useless.

- [x] New module `crates/yserver/src/kms/render/region.rs`: a y-x banded region
  (pixman's structure; `../../../xserver` has the reference in `dix/region.c` if
  the band-merge rules need checking). API, and nothing beyond it:
  `union_with`, `subtract`, `intersect_with`, `intersect_rect`, `add_rect`,
  `contains_rect`, `intersects`, `rects()`, `bounding_rect()`, `area()`,
  `is_empty()`, `clear()`. `intersects` is not sugar — it is how the step-3
  invariant assertion is written, and clone-intersect-is_empty is the version
  that gets subtly wrong.
- [x] Rect count cap as `RegionSet` has, same rationale (bounded `subtract` cost
  in the page-flip handler): above the cap, collapse to extents. Cap at 32 for
  this type — it feeds a scissor list, not a damage log.
- [x] **The cap is only safe in one direction.** Collapsing a *damage* region to
  its extents over-damages, which costs pixels. Collapsing something that is then
  **subtracted** over-subtracts, which leaves stale pixels. So: a capped region
  may be subtracted from `missing` only if that bbox was itself painted — which
  is exactly what the `painted` region of step 3.2 guarantees. Encode it as a
  comment on `subtract` and a test.
- [x] Unit tests: the band invariants (no overlapping rects, no adjacent
  mergeable bands), union/subtract/intersect against a brute-force per-pixel
  oracle on a small grid, subtract-to-empty, subtract of a disjoint region,
  self-subtract, cap collapse.
- [x] **Leave `RegionSet` alone.** The store's presentation-damage path keeps its
  multiset semantics, because that is where an identical rect can legitimately
  arrive twice.
Write this against a brute-force oracle, not against a hypothesis — a wrong
region implementation is indistinguishable from a damage bug at the screen.

## Step 3 — track what each scanout BO is missing

Per output, keyed by `bo_idx` (stable: `bo_generations[output][bo]`,
`SCANOUT_POOL_DEPTH = 3`).

**Invariant: `missing[bo]` is the set of pixels of `bo` that do not reflect the
current scene.** Every operation below is checkable against that one sentence.

### 3.1 State — **DONE** (2026-09-02)

`OutputSceneState.damage: ScanoutDamage`, sized in `build_output_state` from the
same `bo_depth` the buffer-age ring already derived from the live pool. The tick
feeds it at the point `output_damage` is final (after the empty-damage block),
queries `repaint_for` after acquire, and stages on submit *success*; retirement
applies it, the identity mismatch invalidates. `drain_all` invalidates every
output, and `invalidate_all_scanout_damage` covers the two backend sites.
Shared-only: the copied path renders Full unconditionally and accumulates
nothing. Landed against always-Full, so `painted` is the whole output and
nothing on screen depends on it yet.

One addition not in the plan: the damage fed in is **clipped to the output
extent**. Damage outside it cannot be presented, and letting it through would
trip `commit_submitted`'s "painted covers repaint" assertion — which compares
against the full-output rect — turning a stray rect from some producer into a
debug-build panic on hardware.



- [x] In `OutputSceneState` (scene.rs:435):
  - `pending: Region` — damage accrued, not yet attributed to any BO.
  - `missing: Vec<Region>` — one per BO, sized from the scanout pool in
    `build_output_state`, every entry initialised to the **full output**.
  - `in_flight: Option<InFlightDamage { bo_idx, submitted_pending: Region,
    painted: Region }>`.
  Created only for `OutputScanout::Shared` outputs (see 3.4).
- [x] **Cross the `RegionSet` → `Region` boundary here.** Step 0 landed the type;
  this is its first consumer. `output_damage` is assembled from four `RegionSet`
  producers and all four are the boundary — converting only the obvious one
  leaves the rest silently on the old type:
  - `built.projected_damage`, filled by `add_projected_damage` (scene.rs:5004)
    and read at scene.rs:3447;
  - `cursor_damage_for_frame`, which returns a `RegionSet` (scene.rs:3265-3272);
  - the `scene_structure_damage` snapshot (scene.rs:3366-3371, folded at 3460),
    written by `mark_scene_structure_damage_rects` (scene.rs:1252-1269);
  - the `pending_repaint_after_failed_submit` snapshot (scene.rs:3461) — which
    3.5 retires outright, so this one converts only if it outlives that.

  `run_damage_audit` (scene.rs:2665-2672) takes the same value and is part of
  the migration; see 4.7.
- [ ] `.bounding_rect()` is the call to audit while migrating, not `.rects()`.
  The two `.rects()` reads in the tick (scene.rs:3579, 3660) are skip-log counts
  and are unaffected by band coalescing. `.bounding_rect()` carries semantics: it
  is what the audit classifies on and what the old failure path widens to.
- [ ] Keep `BufferAgeRing`, `damage_history`, `submitted_output_damage` in place
  during 3.1-3.3 so the change is additive; delete them in 3.5 once the new model
  drives the pick.

### 3.2 Transitions

- [x] **New damage `D`** → `pending |= D`. Fed from `output_damage` **at the
  point it is final — after the empty-damage block, immediately before
  `acquire_scanout_bo`** (scene.rs:3569), not where it is first assembled at
  scene.rs:3461. The block between the two injects a full-output rect when a
  drawable carries real damage whose projection landed empty (the xfce submenu
  case, scene.rs:3467-3565); feeding `pending` before it would drop that
  injection, and those snapshots only ack through the `PendingAck` the compose
  path builds — so they would never retire and the scheduler would spin.
- [x] **At acquire of BO X** → `repaint = pending ∪ missing[X]`; if
  `token.content_invalidated || token.last_present_generation.is_none()`,
  `repaint = full output` (and step 4 renders Full — see 4.2, the BO has no
  loadable content).
- [x] **At submit** → move, do not copy: `in_flight = Some((X, take(pending),
  painted))`. `pending` is now empty and collects damage arriving during flight.
- [x] **`painted` is what the recorder actually covered, not what was asked for.**
  This is the field the retire subtracts, and getting it wrong is the one way this
  model leaves stale pixels rather than merely over-repainting:
  - bbox rendering (4.3) → `painted` = the bbox, a **superset** of `repaint`;
  - threshold-forced or guard-forced Full (4.2) → `painted` = the whole output;
  - per-rect rendering (4.5) → `painted` = the union of the rects actually passed
    to a render pass.
  Compute it where the `Repaint` is decided, hand it to the recorder, and stage
  the same value. Never stage a region wider than what was recorded.
- [x] **At successful retire** → for every `Y != X`: `missing[Y] |=
  submitted_pending`; then `missing[X] -= painted`; then `in_flight = None`.
- [x] **On failure** → `pending |= submitted_pending`; `in_flight = None`;
  `missing` untouched.

**This replaces the design's `pending.subtract(submitted_pending)` and is not a
weakening of it.** The design's formulation leans on `RegionSet`'s multiset
subtract to keep damage that arrived between submit and retire. With a real
region, geometric subtraction would delete exactly that damage wherever it
overlaps what was submitted. Staging and restoring is the same guarantee without
depending on rect identity: at most one frame is in flight per output (the
`pending_acks` gate at scene.rs:3346 returns early otherwise), so `in_flight` is
a single slot, and the failure path is a union rather than an unwind.

`missing[X] -= submitted_repaint` stays a geometric subtract, and is correct:
what X now shows is the scene as of submit. Damage that arrived after submit sits
in the new `pending` and reaches X through `missing[Y] |= submitted_pending` at
the *next* frame's retire, where X is one of the `Y`.

### 3.3 Land it against always-Full rendering

**The model itself is DONE** — `scanout_damage.rs`, `ScanoutDamage`, 2026-09-02.
Built as a standalone state machine rather than as fields and methods scattered
through `tick_one_output`, precisely so the contract could be proven before any
pixels depend on it. 18 tests, including a 300-step rotation test against an
independent per-BO "owed pixels" shadow model that shares no code with the
implementation. Two deviations from 3.2 as written, both simplifications:

- **Staging happens on submit *success*, not on submit.** An attempt that fails
  then never staged, `pending` was never taken, and there is nothing to restore —
  the next tick recomputes an identical repaint. `retire_failure` remains, for
  the failure paths that land *after* a successful submit (copied-scanout
  completion, teardown discarding a staged frame).
- **`repaint_for` accounts for a frame staged against another BO.** Damage leaves
  `pending` at submit and only reaches `missing` at retirement, so between the
  two there is a window where another BO's debt is invisible. The flip-pending
  gate means it cannot currently be reached; it is handled anyway so the model is
  correct on its own terms rather than on a caller's discipline.

`commit_submitted` carries the two assertions that would have caught round 6's
blockers: `painted` must contain `repaint`, and nothing may be staged twice.

- [ ] Wire all of 3.1-3.2 while `pick_repaint_region` still returns
  `Repaint::Full`. **What this proves is that the model breaks nothing — not that
  the model is right.** Under always-Full, `painted` is the whole output every
  frame, so `missing[X] -= painted` empties trivially and
  `missing[Y] |= submitted_pending` is the only transition with any content. An
  integration run here is vacuous in exactly the way the audit's `full`
  comparisons were.
- [x] **The contract is proven by unit tests against the state machine directly**,
  with partial `repaint` and partial `painted` and BO reuse — not through the
  renderer. Write those first; they are what makes step 4 safe to switch on.
- [x] Debug assertion, enabled in tests and debug builds: after each retire, for
  the BO just presented, `!missing[X].intersects(&painted)`. Note this assertion
  is near-tautological until step 4 lands — it earns its keep afterwards.
- [x] Unit tests over a 3-BO output driving the transitions directly: damage →
  acquire → submit → retire; damage → acquire → submit → **failure** → next
  acquire recomputes the same repaint; damage arriving between submit and retire
  survives into the next frame; an untracked/invalidated BO yields full damage;
  a BO not acquired for several frames accumulates the union of everything.
- [ ] Multi-output: state is per `OutputSceneState` already; add one test that
  output A's retire does not touch output B's `missing`.
- [x] `debug_assert!(in_flight.is_none())` at submit. It holds today — submit
  requires `pending_acks` empty (scene.rs:3346), success clears `in_flight` at
  retire and failure clears it inline — but it is the invariant the whole model
  rests on, so let it fail loudly rather than silently double-stage.

### 3.4 The safe fallback, and every path that needs it

`missing[bo] = full output` is always safe: it costs one full repaint and can
never show a stale pixel. `ScanoutDamage::invalidate` is that escape hatch.

**The call-site list is short — shorter than an earlier draft of this plan
claimed — because two mechanisms already cover most of it.** Mapped against the
code 2026-09-02:

- **`content_invalidated` already covers the BO-level failures.** The failure arm
  of `tick_one_output` calls `platform.invalidate_bo` (scene.rs:3835), which sets
  `content_invalidated` and clears `last_present_generation`. The next acquire of
  that BO therefore reports `loadable = false` and `repaint_for` returns the full
  output on its own. So `invalidate_bo` (platform.rs:5220),
  `cancel_scanout_bo_recording` (platform.rs:5281) and
  `retire_failed_submit_bos` (scene.rs:2337) need **no damage-state call at
  all** — and `retire_failed_submit_bos` in particular must not get one: it runs
  at the top of every `tick_one_output`, *before* the flip-pending gate
  (scene.rs:3341-3343), so an unconditional invalidate there would force a full
  repaint every tick and silently delete the entire win.
- **`rebuild_outputs` already covers topology.** It replaces every
  `OutputSceneState` wholesale (scene.rs:1089), so a fresh `ScanoutDamage` sized
  from the current pool falls out for free — which is why `build_output_state`
  must size `missing` from `platform.scanout_pools.get(i)`, exactly as it already
  derives `bo_depth` (scene.rs:1043-1047). Every path that changes pool length or
  output indices is followed by `rebuild_outputs` in the same call chain:
  `enable_connector_inner`'s pool replacement (platform.rs:6044-6059) via
  `apply_crtc_config` (backend.rs:16890) and `finish_crtc_config`
  (backend.rs:16573); `remove_connector_at`, which shrinks four parallel vectors
  and renumbers every later output (platform.rs:5415-5449), via the same
  `apply_crtc_config` path and via `apply_connector_snapshot`'s descending-index
  loop under `fire_randr_changes` (backend.rs:9643), whose `rebuild_scene` flag
  is computed from the same `preserves_active_output` predicate that decides
  whether anything was dropped. So removal and rebuild are paired by
  construction.

That leaves **four explicit calls**:

- [x] **`drain_all`** (scene.rs:1456-1566), after the loop, unconditionally and
  for every output — it takes no `output_idx` and iterates them all. It pops
  every ack, and **retains** any whose fence wait failed (scene.rs:1497-1506), so
  a staged frame can be discarded or left half-retired. Invalidating everything
  is consistent with both. This one call also covers **suspend**
  (`reset_scanout_bos_for_suspend`, platform.rs:5311), **DPMS off/on**
  (`dpms_set_outputs_active`, platform.rs:6363) and the **topology quiesce**,
  because all three run `drain_all` first in the same function
  (backend.rs:2036, 9408, 23641).
- [x] **The page-flip identity mismatch** (scene.rs:1703-1712).
  `platform.on_page_flip_complete` has already advanced the BO phase machine —
  previous `OnScreen` → `Free`, `Pending` → `OnScreen` (platform.rs:6194-6205) —
  by the time the ack is tested, and the mismatch branch then returns without
  popping it. Platform and scene have diverged; neither `missing` nor the staged
  frame can be trusted. (Note the ordering: `expected` at scene.rs:1692-1696 is a
  read-only peek, and if `on_page_flip_complete` returns `None` the function
  returns at 1697 before the mismatch check is reached.)
- [x] **`set_logical_screen_size`** (backend.rs:16928-17134). RRSetScreenSize
  reallocates root and COW *storage* — not scanout BOs — so BO contents stay
  valid while the composed image beneath them changes size. It deliberately
  avoids `drain_all` + `rebuild_outputs` (comment at backend.rs:17105-17127:
  doing so desynchronises the flip-pending gate from the kernel's DRM event
  queue and froze output 1 black on a live two-monitor session), and it touches
  no scene ledger field at all — its only damage call is `wake_for_damage`
  (backend.rs:17130), which adds no region. So nothing else will tell the model
  the root changed.
- [x] **`retire_direct_output`**, the full-retirement branch (backend.rs:2263-2297,
  alongside the existing `mark_scene_structure_dirty` at 2237). While a CRTC
  scans out a client buffer directly the composed BOs are not being painted and
  the scene is not tracking them. Worse, when this returns `true`
  `handle_page_flip_complete` is **not called at all** for that output
  (backend.rs:14966-14971), so the scene's retire path is bypassed entirely.
- [x] **Copied outputs need nothing.** `handle_scanout_render_completion_inner`'s
  failure path (scene.rs:1898-1953) is `Copied`-only, now confirmed from the
  code: the `Shared` arm maps its result with `.map(|()| None)` (scene.rs:3730),
  `Ok(None)` becomes `InFlightStage::KmsFlipPending` (scene.rs:3779), and only
  the `Copied` arm can produce `Ok(Some(..))` and hence
  `WaitingForRenderCompletion` (scene.rs:3759-3761). Since copied outputs carry
  no `ScanoutDamage` (they render Full unconditionally), there is nothing staged
  for that path to restore. Assert it rather than relying on the reasoning.

One caveat on the coverage claim above: it rests on enumerating callers within
`kms/render/backend.rs` and `kms/render/platform.rs`, where `KmsBackend` and
`PlatformBackend` are defined and used. A caller outside those two files would
not have been seen.

### 3.5 Remove the superseded path

- [ ] Delete `BufferAgeRing`, `damage_history`, the `contains_all` /
  `union_history_into` machinery and their tests once 4.1 consumes the new model.
  Keep `submitted_output_damage` on `PendingAck` only if telemetry still needs it.
- [ ] **Retire `pending_repaint_after_failed_submit` for shared outputs.** `pending`
  does its job exactly and does it better. Keeping both is not merely redundant,
  it is harmful: on a failed submit the new rule restores `submitted_pending`
  *and* the old rule folds `output_damage.bounding_rect()` into the accumulator
  (scene.rs:3866-3868), whose snapshot re-enters `output_damage` next frame
  (scene.rs:3461) — so **one failed submit permanently widens the following
  repaint to a bbox**, on precisely the path where the model is supposed to be
  precise. Copied outputs keep it (they carry no model state, 3.4).
- [ ] **Keep `scene_structure_damage`.** It is not redundant: `mark_scene_structure_dirty`
  writes it from outside a tick (scene.rs:1195-1215), where there is no
  `output_damage` in scope to union into. It can only be retired once step 2 has
  converted every producer to write a region directly, and that is step 2's call,
  not step 3's.
- [ ] Leave the `Copied` exclusion of 3.4 stated in the code, not just here.

## Step 4 — repaint only the damaged region

**4.1-4.4 IMPLEMENTED, 2026-09-02, UNSMOKED.** `plan_repaint` replaces
`pick_repaint_region`: it applies every gate below and returns both the
`Repaint` and the `painted` region the recorder will cover, so what retirement
subtracts is what was actually drawn. `cull_scene_to_rect` produces the render
list as a *separate* product from `built.scene`. `render_area` narrows to the
clipped rect. Telemetry reports painted area, requested-region area, clipped
count, and a per-reason Full-fallback breakdown.

**This is the commit that changes pixels. It has not been near hardware.** 4.5
(multi-rect) is deliberately not built — the plan says only if measured, and the
new `damage_region_fraction` against `damage_fraction` is what measures it. 4.7
(per-BO audit candidates) is still outstanding and is needed before an audit run
counts as evidence for this step.



**This is the step that cuts GPU load.** Everything above is bookkeeping.

### 4.1 Feed the pick from step 3

- [x] `pick_repaint_region` takes `repaint: &Region` and the output extent, and
  returns `Repaint::Full` or `Repaint::Clipped(bbox)`. Remove the disabled
  buffer-age body and its doc comment's re-enable hazards as they are discharged
  below.

### 4.2 The gates that make clipping safe

Each of these is a known way to corrupt the screen; all are already documented in
the code as re-enable hazards.

- [x] **Opaque bottom layer.** Clip only if the repaint bbox is contained in the
  `dst` rect of some draw with `alpha_passthrough == false`. Three ways to get
  this wrong, all of which mean stale pixels:
  - *Evaluate it on the list that is actually recorded* — after the cull of 4.3,
    not on `built.scene.draws`. A containing draw always intersects the scissor
    so culling cannot remove it, but the two lists must not be allowed to drift.
  - *Prove coverage numerically, do not assume an index.* The root draw is
    conditional on the root lookup, `scene_participating`, `DrawableKind::Root`
    and a non-null source view (scene.rs:4095-4108), and its dst is
    `[-layout_x0, -layout_y0]` sized from root **storage** extent
    (scene.rs:4109-4126) — coverage is data-dependent, and a multi-output layout
    with non-zero origins is exactly where it stops being obvious.
  - *Round conservatively.* `dst_origin`/`dst_size` are `f32`; round the origin
    up and the far edge down before testing containment, so a fractional edge
    never counts as covered.
  Note what is **not** an opaque bottom layer: every COW-subtree draw is
  `alpha_passthrough = true` by construction (scene.rs:4291-4294, 4823-4828), and
  so is the SW cursor (scene.rs:4366-4375). A compositing desktop will therefore
  usually fail this gate and render Full — which is correct, and costs nothing,
  because a compositor presents a full-screen surface every frame anyway.
  Fails → Full.
- [x] **Empty draw list** → Full. Already handled (scene.rs:3585); keep it.
- [x] **Unloadable BO** → Full. `content_invalidated` or never presented means
  `loadOp = LOAD` from `UNDEFINED`, which is invalid, not merely stale.
- [x] **Root `IncludeInferiors` XOR overlay.** Non-idempotent: a LOAD frame whose
  scissor misses the overlay rects leaves a previously-XORed BO double-applied
  (the #90 rubber-band remnant). Fold `root_overlay.all_rects()`, clipped to the
  output, into the repaint region before picking.
- [x] **Software cursor — fold the rect only when a SW cursor draw is actually
  in the list being recorded.** A stationary sprite is only present in the BO that
  last drew it, so it must be folded in even on a frame triggered by unrelated
  damage (scene.rs:3455). Two ways to get this wrong:
  - `built.new_cursor_rect` is `Some` for a **HW-plane** cursor too
    (scene.rs:4353-4362). That rect is plane content, not BO content; folding it
    would repaint a region for no reason on every cursor move, on the path where
    the HW plane exists precisely to avoid that.
  - `cursorless_hide_frame_required` → `omit_software_cursor_for_hide`
    (scene.rs:947-956, called at scene.rs:3408-3414) removes the SW draw for the
    Hw→Sw/Hidden handoff frame. Fold **after** that omission, keyed off the draw
    list, not off the assignment computed before it.
- [x] **Damage-fraction threshold, computed on what will be PAINTED.**
  `area(painted_candidate) / area(output) >= 0.6` → Full — where
  `painted_candidate` is the bbox under cut-1 rendering, not the region. Sparse
  damage spread across the screen has a small region area and a bbox covering
  nearly everything; thresholding on the region would pick the clipped path and
  then rasterise the whole screen anyway, with the LOAD and scissor overhead on
  top. That is the 0.857 measurement's failure mode reintroduced through the back
  door.
  Measured: at `mean_damage = 0.857` a clipped compose cost 208.7 µs against
  199.3 µs full, because a sub-rect pass still pays scissor setup and every draw
  call. A named constant, not an env var; re-measure the crossover on the z400
  and adjust once (the bee number is a fast tiled GPU, LINEAR Polaris will cross
  over higher).

### 4.3 The rendering change

- [x] `Repaint::Clipped(rect)`: set `render_area = rect` as well as the scissor.
  It is the full BO today (scene.rs:5548), which asks the driver to load an
  attachment we then refuse to touch.
- [x] **Cull draws that do not intersect the scissor**, before descriptor
  allocation. The audit's fit put a fixed floor of 40-110 µs per compose in draw
  calls, descriptor binds and pipeline switches that clipping alone never
  removes; culling is where that floor comes down. Preserve draw order among the
  survivors, and cull in one place so the descriptor array and the record loop
  cannot disagree — `record_and_submit_render` indexes `descriptors[i]` by draw
  index (scene.rs:5495).
- [x] The audit's reference compose keeps the **unculled** scene; it is Full by
  construction and must stay an independent oracle.
- [x] Cull and allocate from one list. The recorder indexes `descriptors[i]` by
  draw index (scene.rs:5662), so a filtered scene must be built once and handed
  to both the allocation loop and the record loop — never filtered twice.
- [x] **HARD INVARIANT: the culled list is a separate product and never replaces
  `built.scene.draws`.** The full list is what step 1's visibility pass, step 2's
  `prev_draws`, the drawable snapshots and the audit's reference oracle all read.
  Get this backwards and step 2 silently destroys the entire win: every culled
  draw reads as *disappeared* this frame and *appeared* the next, so the diff
  manufactures structural damage covering them, `pending` goes wide, the threshold
  forces Full, and the screen looks perfectly correct while the GPU saving is
  gone. It would present as "clipped repaint doesn't help after all" — the exact
  wrong conclusion, reached silently.

### 4.4 Telemetry that means something

- [x] **Two areas, not one.** `damage_fraction` (telemetry.rs:444) is
  `damaged_pixels / output_pixels` from `record_damage_pixels`
  (telemetry.rs:818). Feed it `area(painted)` — what was actually rasterised,
  which is the number that tracks GPU cost and stops it being 1.000 by
  construction. Add a second counter for `area(damage_region)`. The **gap between
  them is bbox waste**, and it is precisely the input to the 4.5 multi-rect
  decision, so collecting only one of the two leaves that decision unmeasurable.
- [x] Add `clipped_repaint` alongside `full_redraw_fallback`, and a counter for
  each Full-fallback reason (no opaque cover / empty draws / unloadable BO /
  threshold). A regression here shows up as a reason count, not as a mystery.
- [ ] `avg_gpu_render_ns` is already wired from the timestamp pool and is the
  before/after number for production composes — same buffers, same layout, so it
  compares against itself honestly.

### 4.5 Multi-rect rendering — **IMPLEMENTED AND CONFIRMED** (2026-09-02)

Phased workload on silence, before and after, identical scripted input
(`../findings/data/2026-09-02-step45-workload-silence-*.log`):

| phase | painted before | painted after | region | gpu before | gpu after | full/s |
|---|---|---|---|---|---|---|
| idle | 0.141 | 0.141 | 0.141 | 23.9 µs | 26.3 µs | 0 → 0 |
| **drag** | **0.268** | **0.171** | **0.171** | **52.6 µs** | **33.8 µs** | **8 → 0** |
| resize | 0.190 | 0.160 | 0.160 | 38.2 µs | 36.7 µs | 0 → 0 |

Painted now equals the requested region in **every** phase, so bounding-box
waste is gone. The drag phase paid 36% for the box and now pays nothing; its GPU
per compose fell 36%; and its 8 Full frames/s went to zero, confirming those were
threshold hits on an inflated box rather than on real damage. Idle is unchanged,
so single-rect frames pay nothing for the feature.

Cost: `cb_record` in the drag phase rose ~5% (134.6 → 140.9 µs) from the
scissor-major replay issuing some draws twice. Bounded by `MAX_SCISSOR_RECTS`.



`plan_repaint` returns a scissor list: the damage region's own rects when the
bounding box wastes more than `MULTI_RECT_MIN_GAIN` (1.5, set below the 1.57 the
drag phase measured) and there are at most `MAX_SCISSOR_RECTS` (8) of them,
otherwise a single box as before. The recorder is scissor-major — for each rect,
replay the draws that touch it — and `Repaint::Clipped` still carries the box,
which is now the *render area* rather than the scissor.

Two consequences worth knowing:

- **The threshold moved.** It is measured on `painted`, which under per-rect
  rendering excludes the gap. So a frame whose box is over the line but whose
  rects are well under it now stays clipped instead of repainting the whole
  output — which is where the drag phase's 8 Full frames/s should go. The
  step-4 test asserting the box rule was superseded and rewritten to pin both
  halves.
- **Scissor disjointness is load-bearing**, not incidental: the root XOR overlay
  is not idempotent, so each of its pixels must fall in exactly one scissor. The
  rects come from a canonical `Region`, which guarantees it; a test pins it
  across a band boundary, where a hand-rolled rect list would overlap.



**First conclusion was wrong.** Whole-session medians put bounding-box waste at
0.5-1.8% and this section said "do not build". Those medians are dominated by
idle frames, where damage is one contiguous rect from a playing video and the
bbox is free.

The phased workload (`tools/damage-workload.sh`) separates the populations, and
in the **drag** phase the waste is **36%** — painted 0.268 against a requested
region of 0.171
(`../findings/data/2026-09-02-step2-workload-silence-phases.log`). Obvious in
hindsight: a moved window's damage is *exactly two disjoint rects*, old and new,
and their bounding box spans both plus the empty gap between them. Idle 0%, drag
36%, resize 19%.

So per-rect rendering is worth building, and it is worth building **after** step
2 rather than before, because step 2 is what creates the two-rect shape in the
first place. It should also recover the 8 Full frames/s the drag phase still
loses to the threshold, since those are threshold hits on an inflated bbox
rather than on real damage.

The lesson is about method, not about rects: **a median over a whole session
cannot answer a question about one kind of frame.** That is the same error as
the step-2 magnitude attempt on the same day.

### 4.5 Multi-rect rendering (superseded by the measurement above)

Cut 1 renders the bbox. Take this only if the telemetry shows the bbox costing
real pixels — the shape to watch is a panel clock plus a video window in one
frame, where the bbox is most of the screen and the rects are 8% of it.

- [ ] Render per rect when `rect_count <= 8` and `area(bbox) > 1.5 * area(rects)`:
  loop the rects, `cmd_set_scissor` per rect, redraw the draws intersecting it.
  Otherwise bbox.
- [ ] `painted` = the union of the rects rendered, per step 3.2. The
  `ScanoutDamage` model already accepts any region here, so this is a recorder
  change only. If this is ever
  left as the bbox while the passes are sparse, `missing[X]` is cleared for holes
  that were never drawn — the one failure mode that shows as stale pixels rather
  than wasted work.
- [ ] **The XOR overlay constrains how the passes may be split.** Overlay ops set
  their own scissors inside the compose (vk/ops/scanout_logic_fill.rs:69) but are
  still clipped by the pass `render_area`, so an op outside a narrowed area is
  silently dropped, and one spanning two areas must not be applied twice. Both are
  satisfied because the region's rects are **disjoint by construction** and the
  overlay rects are folded into the region (4.2): each overlay pixel falls in
  exactly one pass. That is load-bearing on the region type's band invariant —
  test it directly with an overlay op straddling a band boundary.
- [ ] Keep the accounting on rects either way, so the decision has data behind it.

### 4.6 Why the historical drag-shake does not come back here

Clipped repaint was reverted before because of multi-pixel "drag-shake" on
non-composited MATE: stale drag-phase content in BOs the catch-up scissor did not
cover. It was never root-caused. Two things make step 4 a different proposition:

- Staleness is now tracked **per BO against the scene**, not reconstructed from a
  generation window (`contains_all` over a ring pushed only at retire). There is
  no window to be wrong about.
- **Drag is still whole-output damage.** `mark_scene_structure_dirty` fires on
  configure, so drags take the Full path under the threshold. Step 4 changes
  nothing about window management.

Which also says where the risk actually lives: **step 2 is what makes drag
partial again**, and step 2 is where that ghost has to be faced. Note that the
audit cannot pre-clear it — window management damages the whole output by
construction, so those frames classify `full` and prove nothing
(scene.rs:541-546).

### 4.7 Make the audit test this model, because today it cannot

**The audit measures damage producers, not repaint correctness, and the plan must
not claim otherwise.** It keeps **one** persistent candidate image per output
(`OutputDamageAudit.candidate`, scene.rs:514), updates it inside
`output_damage.bounding_rect()` and compares against a full reference. So it
answers "does `output_damage` cover everything that changed" — a producer
question, and the right regression test for steps 1 and 2.

It cannot answer the step-3/4 question at all. Production repaints
`pending ∪ missing[X]` into **one of three rotating BOs**; a bug in `missing[]`
shows up as a stale pixel in the BO that was not repainted, and a single
candidate image has no way to represent that. Nor does `mean_damage`
(scene.rs:2500, from the same bbox at scene.rs:2838-2846) still estimate the
prize: production now paints `painted`, which is a superset. Two consequences,
both of which must be written into the plan and the code:

- [ ] **`mean_damage` is a producer metric from here on.** It stays the signal for
  "did step 2 make drag partial", and it stops being a proxy for GPU cost. The
  cost signal is telemetry's `damage_fraction` fed from `area(painted)` (4.4).
  Anywhere the acceptance section leans on `mean_damage` for cost, it is leaning
  on the wrong number.
- [ ] **Extend the audit to one candidate per BO** — three images instead of one,
  rotated in the same order production rotates, each updated only with the
  `painted` region of the frame in which production acquired that BO, and compared
  against a full reference whenever that BO is acquired again. That is a faithful
  simulation of the per-BO model, it reuses the compare pipeline and the
  `idle`/`partial`/`full` classification unchanged, and it is the only thing that
  catches a `missing[]` bug before a user does. Cost is two more full-output
  images per output on a diag-gated path, which is affordable for exactly the
  reason the original audit was.
- [ ] Pass `run_damage_audit` both regions — producer damage and production's
  `painted` — rather than the single `output_damage` it takes today
  (scene.rs:2665-2672). Passing one and inferring the other is how the stale
  metric survives the refactor.

### 4.8 Gates

**First run: awesome on silence, non-composited, PASSED functionally.**
114 telemetry buckets, no panic and no assertion failure with
`-C debug-assertions=yes`; nothing visually wrong with a few terminals and
windowed mpv. **2451 clipped frames (72.3%)**, painted fraction median 0.449.
Every one of the 941 Full fallbacks was `threshold`; `no_opaque_cover`,
`unloadable_bo` and `empty_draws` were **zero across the whole run** — so the
opaque-cover guard found its covering draw on every clipped frame, and
`loadable` tracked BO state correctly across rotation. Those were the two gates
whose failure mode is stale pixels.

Still outstanding: a master baseline for the before/after ratio (needs a separate
worktree with its own `CARGO_TARGET_DIR`), MATE non-composited (~4 draws per
compose on awesome means the draw cull was barely exercised), and the composited
negative control.


- [x] Unit tests for every gate in 4.2, including a scene whose bottom draw is
  `alpha_passthrough = true` (COW) forcing Full.
- [ ] Audit clean at `interval=1` with a non-trivial `partial` count, on MATE and
  Window Maker — **with the per-BO candidates of 4.7**, not the single-candidate
  version, which cannot fail on a `missing[]` bug.
- [ ] The run must actually exercise the thing being tested: production took
  `Repaint::Clipped` on a non-trivial share of frames, with a **non-empty
  `missing[X]` contribution** on some of them (i.e. BO reuse after several frames
  of damage), not merely `pending`. A clean run where every repaint came from
  `pending` alone has not tested the per-BO model. Add the counter that makes this
  observable.
- [ ] Eyes-on smoke under lightdm→yserver, not the bare `just *-hw*` harness
  (it starts a session without keyring/systemd-user context and manufactures app
  hangs): MATE menus, drag, resize; awesome; i3 floating drag; shaped windows (marco rounded corners);
  root rubber-band selection over a repainting window; VT switch and DPMS
  round-trip; a fullscreen unredirected window over the COW.
- [ ] **The measure:** GPU utilisation on the z400 + RX 460, windowed mpv, same
  clip, against labwc's 2.8% on the same box. Windowed on both sides — fullscreen
  invites direct scanout and stops measuring compositing. Read total GPU
  utilisation, not per-frame accounting.
- [ ] No regression against `ad318afa` on the Full path.
- [ ] **Instrument `build_scene` first — it is the one unmeasured half.**
  `compose_cb_record_ns` starts *after* the walk (scene.rs:3710, `record_start`),
  so it covers descriptor allocation and CB recording but not the scene walk. The
  walk is where step 1's visibility pass would go, and it already contains an
  O(N²) shape: `emit_window_subtree` scans the whole `WindowsMap` and allocates
  and sorts a `Vec` **per visited node** to find its children (scene.rs:4905-4917).
  Add a timer around `build_scene` before adding work to it; if the child scan
  shows up, a parent→children index is a small, local fix worth taking on its own
  merits.
- [ ] **CPU must not absorb the GPU win.** Both sides are already instrumented:
  `avg_gpu_render_ns` is the GPU compose, `avg_compose_cb_record_ns` is the
  CPU-side recording, and `descriptor_allocations/s` counts the per-draw work that
  dominates it. Record all three before and after, on the z400, on the same
  workload. The bar: **`avg_compose_cb_record_ns` must not rise.** It should
  *fall*, because culling draws outside the scissor (4.3) removes exactly the
  `vkAllocateDescriptorSets` / `vkUpdateDescriptorSets` / bind / push-constant
  sequence that makes it up.
- [ ] **Watch latency, not just throughput.** The render tick shares its thread
  with protocol processing, so CPU added per frame is taken from client request
  handling. A change that improves GPU utilisation while making the desktop feel
  worse would pass every other gate in this section. Interactive smoke — menus,
  drag, typing latency — is the instrument here, and it is why the eyes-on gate
  above is not optional.

## Step 2 — damage falls out of scene changes

**2a + 2b IMPLEMENTED, 2026-09-02, UNSMOKED.** `scene_diff.rs` holds the diff;
`SceneBuild.participants` carries one presence per emitting participant with its
region derived *from* the draws it pushed, so the two cannot drift.
`OutputSceneState.prev_presented` advances only at retirement. 16 of the 21
`mark_scene_structure_dirty()` sites are demoted to `wake_for_damage()`; the
5 left coarse are `finish_cow_release`, `retire_direct_output`, the degraded
composed unflip in `maybe_composite`, `get_overlay_window` — CRTC and overlay
ownership transitions — and `set_window_scene_participation`, whose hammer is
only the no-geometry fallback of an otherwise region-precise path.

`structural_fraction` in the telemetry separates the two ways this can
disappoint: high on quiet frames means the diff is churning; ~0 alongside a high
`damage_fraction` means a mutator is still posting whole-output damage.

**First run (silence, MATE non-composited, deliberate dragging): mechanism
confirmed, magnitude NOT measured.** No panic or assertion failure across 60
buckets. `structural_fraction` median **0.001** — the diff is quiet, so the
identity and signature comparisons are stable frame to frame and 2a is not
churning. Clipped share **68.7% → 87.2%**, Full frames/s median **7 → 3**, so
structural changes now clip instead of hammering. `no_opaque_cover` still 0.

The GPU magnitude is **unmeasurable on this hardware**, and not for the reason
first recorded here — the sessions were comparable in what was done, so blaming
a drag-heavy workload was wrong. Binning both runs by `paint_submits/s` gives no
consistent signal at all (−63%, −17%, +14%, +34% across four bins of 12-45
samples). Two human-driven desktop sessions differ in more dimensions than
`paint_submits/s` captures — window count, what is on screen, mpv's size and
position, panel activity — and on an RX 6800 the per-frame stake is 100-200 µs,
which is **smaller than the between-session variance**. More A/B runs on silence
cannot fix that.

The mechanism still rules out a regression: the diff can only produce less
damage than the whole-output rect it replaced, and `structural_fraction`
confirms it produces very little.

**Measure it on the z400**, where a full compose is ~5 ms, so converting a drag
frame from 100% to ~31% is ~3.5 ms — 20-30× the noise floor rather than below
it. And **script the input** (`xdotool` moving a window along a fixed path for a
fixed time, same clip playing) so both branches see identical events; that is
what removes the session variance this pair of runs foundered on.

**MATE run, 2026-09-02: 4.5 does not engage there.** Drag painted **0.255**
against a region of **0.169** — 34% bounding-box waste, back again, because
MATE's panels and desktop fragment the damage region past
`MAX_SCISSOR_RECTS = 8` and it falls back to the box. On awesome the drag region
was two rects. Drag also still loses 7 frames/s to the threshold, which that box
inflation causes. **FIXED the same day: `MAX_SCISSOR_RECTS` is now `Region::MAX_RECTS`, so it
never binds.** The 8-rect cap existed to bound draw calls, on the reasoning that
each scissor re-issues every intersecting draw. That reasoning used the wrong
number: the count that matters is the **post-cull** draw count, measured at 4.0
per compose against 53.6 pre-cull, because the scissor cull removes 92% of draws
when damage is a small fraction of the screen. So the cost is scissors × ~4, and
a fragmented region is affordable. `Region` already collapses to its extents
above its own cap, so the list is bounded regardless.

Worth noting how this was nearly missed: on awesome, a tiling WM, the drag region
is two rects and the cap never bit, so the tiling measurement looked perfect
while the realistic desktop silently lost the whole benefit.

**Confirmed on MATE** (`../findings/data/2026-09-02-capfix-mate-silence-*.log`):
drag painted **0.255 → 0.169**, exactly the requested region, GPU 65.1 → 47.8 µs
(−27%, clear of the ±25% noise floor and corroborated by the deterministic area
metric); resize 0.184 → 0.164. **Full frames/s went to zero in every phase** —
every `full_reason` counter reads 0 across the run, so on a realistic
non-composited desktop every frame now takes the clipped path.

**Phased-workload run (silence, awesome non-composited, scripted input):
deterministic and clean.** No panics. The three idle phases report painted
**0.141** and structural **0.000** identically, so the workload has no drift and
the phase comparisons are sound. `drag` painted 0.268 / structural 0.015, gpu
53 µs against idle's 24 µs; `resize` painted 0.190 / structural 0.011.

**`restack` recorded nothing** — identical to idle in every column, almost
certainly awesome declining `windowraise`/`windowlower` for tiled windows, which
do not overlap. So the diff's **rank-comparison path is still untested on
hardware** and has unit coverage only. Drive it with a floating layout or
deliberately overlapping windows before trusting it.

**2c IMPLEMENTED and confirmed inert here, 2026-09-02.** The phased workload
reports `painted` identical to three decimals in all six phases, which is what a
no-op should look like: the coordinate fix only bites on a multi-output layout
with a non-zero origin, and the redirect wakes need a compositor. Two parts, and the mutator list turned out to be
**two sites, not five** — checked rather than taken from this plan:

- **The coordinate-space fix.** `dispatch_clip_rects_to_outputs` now subtracts
  the output's layout origin before clipping, with the origin cached on
  `OutputSceneState` next to the extent (the marking entry points are on
  `SceneCompositor` and have no `PlatformBackend` in scope). Every caller passes
  root-absolute rects — `window_absolute_rect` for the participation path, and
  the root overlay's own root-absolute rects — while the clipper worked in
  output-local space. Corroborated by an internal inconsistency: the overlay's
  *rendering* path already translates (`apply_list_for_output`), so the two
  halves disagreed. Invisible on a single output at (0,0); on a multi-output
  layout the damage landed on the wrong output or vanished. A test now pins two
  side-by-side outputs.
- **Wakes on `allocate_redirected_backing` and `release_redirected_backing`.**
  Redirecting or un-redirecting changes what the host window samples, which the
  diff reads as a signature change — but nothing scheduled a tick, so that
  damage waited on an unrelated event. Masked in practice because the callers
  are usually destroying the window too and `destroy_subwindow` wakes; "usually
  masked" is not a guarantee.

**The other three on the old list did not need anything, and one would have been
wrong:**

- `sync_window_leaf_storage_to_geometry` — its only caller is
  `configure_subwindow`, which wakes (2b).
- `set_logical_screen_size` — already wakes, and now also invalidates (3.4).
- `set_backing_scene_participation` — **must not** wake. A backing is a pixmap
  with no on-screen geometry, so it is not a scene participant; the host
  window's participation call owns the geometric damage. This is pinned by
  `set_backing_scene_participation_flips_flag_no_damage`, whose reasoning is
  correct and which adding a wake would have broken.



The extension that makes drag, resize and menus cheap — the frames that are
whole-output *by construction* today, and therefore the ones step 4 alone cannot
help. Lands after step 4 (which is what makes tight damage worth producing) and
before step 1 (which it does not need).

### The design's mechanism does not fit this server, and here is why

The design copies wlroots' `scene_node_update`: mutators assign, then call one
function that takes the node's *cached previous* visible region, unions its new
bounds, re-derives visibility for everything in that region, and posts old ∪ new.
Every part of that rests on a **persistent scene graph** — `wlr_scene_node`
objects that live between frames and carry `node->visible`.

**yserver has no such thing.** `build_scene` (scene.rs:4058) is a *stateless*
walk: `tick_one_output` calls it fresh per output per frame (scene.rs:3390), it
reads `top_level_order`, the `WindowsMap`, `shape_bounding` and the store, builds
`draws`/`snapshots`/`projected_damage` as locals, and returns a `SceneBuild`
(scene.rs:925) that is consumed and dropped. The scene graph is *reconstructed
from the windows map every frame*. There is no node to hang a cached region on.

Building one is the obvious response and it is the wrong one. It means a
persistent per-output cache that must be invalidated by **every** input the walk
reads, and that list is long and includes paths that call no damage function at
all today:

| input | mutated at | damage call today |
|---|---|---|
| `top_level_order` | `sync_top_level_order` (backend.rs:14772) — replaces the whole vector | full |
| `WindowsMap` geometry/mapped/parent/stack_rank | create/map/unmap/configure/reparent/register | full |
| `shape_bounding` | `set_shape_rectangles` (backend.rs:23197) | full |
| `scene_participating` | `set_window_scene_participation` (backend.rs:17636) | rects |
| " (on backings) | backend.rs:17672 | **none** |
| `redirected_target` | redirect alloc/release (backend.rs:17767, 17914) | **none** |
| storage `image_view` / extent | storage realloc on resize (backend.rs:3295-3339), logical resize (backend.rs:16958-17030) | **none** |
| `cow_host_xid` | overlay get/release (backend.rs:18000-18091) | full |
| per-output layout | `rebuild_outputs` (scene.rs:1080) | rebuild |
| cursor position | HW fast path (backend.rs:10278, 10348) | **none** |

Five of those mutate walk inputs with no scene-damage call. A cache invalidated
by hand across that surface will be wrong, and its failure mode is a stale pixel
— the exact bug class this project exists to remove. Adding a second source of
truth for the scene, to be kept in sync with the windows map by hand, is how this
work would fail.

### The mechanism that does fit: diff the emitted scene, at tick time

`build_scene` already runs every frame, before the damage decision, and its
output already *is* the scene: a flat, ordered list of draws in output-local
coordinates. So keep the previous frame's list and diff against it.

- [x] **Where it runs, and this is load-bearing: immediately after `build_scene`,
  and its damage is unioned into `output_damage` BEFORE the empty-damage check at
  scene.rs:3467.** The demoted sites only `wake_for_damage()`, which sets a flag
  and adds no region (scene.rs:1183). So if the diff lands after that check, a map
  or unmap or restack wakes the tick, the tick finds `output_damage` empty, takes
  the EmptyDamage skip and returns — and the window never appears on screen. Not a
  performance regression: a functional break, on the most common operation there
  is. Fold the diff in at scene.rs:3444-3461 alongside the other producers.
- [x] Per output, retain `prev_draws` — the participant key, dst rect, source
  rect, sample view, alpha flag and derived `visible` region of every draw in the
  last **successfully presented** frame, taken from the **full pre-cull list**.
- [x] The retained view handle is `Storage::sample_view` — the swizzled sample-side
  view the scene actually binds (scene.rs:4121, 4787), not the raw
  `storage.image_view`. They alias the same image and the plan must not leave the
  reader to guess (store.rs:127).
- [x] **The participant key needs a generation, not just xid + role.** An xid can
  be destroyed and reused, and Vulkan handles are recycled too, so a
  destroyed-and-recreated participant at the same geometry can compare *equal* to
  the one it replaced — and the diff then reports no change for what is a
  different window. Carry the `DrawableId` (or a store generation) in the retained
  metadata and compare it.
- [x] **Aggregate by participant before comparing.** A shaped window emits one
  draw per shape rect (scene.rs:4793-4807), so "changed geometry" is a property of
  the participant's whole draw sequence, not of any single quad. Union each
  participant's rects (and visible regions) first, then diff the aggregates.
- [x] Structural damage = for every participant whose entry **appeared**,
  **disappeared**, **changed geometry or source**, or **changed stacking order**:
  its old visible ∪ its new visible. Union across participants. That is the whole
  mechanism.
- [x] **This does not need step 1, which is why it goes first.** Without visible
  regions the diff uses each draw's `dst` rect — a superset, so correct, just
  wider wherever something is occluded. Step 1 then tightens it by swapping the
  rect for the region, one line at the diff site.
- [x] Advance `prev_draws` **transactionally**, on successful retire only, staged
  on `PendingAck` exactly like `painted` and `missing` (3.2). A failed submit must
  leave the old list in place or the structural damage is lost — the same rule
  that governs everything else in this design.
- [x] Write the skip paths down; `build_scene` runs at every one of them, so each
  needs an answer:
  - **EmptyDamage** (scene.rs:3467) — reachable only if the diff was empty, since
    the diff is folded in before it. `prev_draws` stays at the last presented
    frame. Correct.
  - **NoBO** (scene.rs:3570) and **NoPool** (scene.rs:3648) — both return before
    any `PendingAck`, so `prev_draws` must not advance; the next tick re-derives
    the identical diff against the same `prev_draws` and unions it into `pending`
    again, which is idempotent.
  - **Submit/commit failure** (scene.rs:3849) — no ack, no generation advance,
    `prev_draws` stays old, `pending` keeps the damage via the 3.2 restore.

Why this is better here, not merely different:

- **The invalidation problem disappears.** Every input in the table above feeds
  `build_scene`, so every one of them shows up in the diff automatically. The five
  mutators that call no damage function today need only `wake_for_damage()` — a
  wake, not a region — and they get correct damage for free. There is no second
  source of truth, because the "cache" is a snapshot of the render input.
- **It replaces the hammer with a smaller change, not a bigger one.** Step 2
  becomes: *demote* the ~20 `mark_scene_structure_dirty()` calls to
  `wake_for_damage()` and let the tick derive the region. Compare that with
  threading `core`, `store`, `windows`, `platform` and `cow_host_xid` into forty
  protocol-time call sites so each can run an output-aware update.
- **It handles multi-window mutations for free.** `sync_top_level_order` replaces
  the entire order vector, and `reparent_subwindow` changes parent, position and
  stack rank and then depends on a later reproject (backend.rs:17418-17428). A
  mutation-time model needs a transaction spanning every affected window, per
  output; a diff does not care how many things changed.
- **It reproduces the design's invariant naturally.** For a drag: the moved
  window's entry changed, so damage is its old ∪ new — and the content revealed
  beneath it lies inside the old rect, so the draws below need no damage of their
  own. That is exactly the design's "posting only the mutated node's old ∪ new is
  sufficient", arrived at by construction instead of by argument.

What it costs, stated honestly:

- One retained draw-metadata list per output. Small — the draw struct is a handle
  and a few floats — against a 3-6 ms compose, but it is per frame, so measure it.
- **Over-damage on reorder.** Index-wise comparison flags a whole suffix when
  stacking changes. Key the diff by participant and compare sets, not positions,
  so a pure reorder damages only the reordered participants' rects. Restacks are
  visually large anyway; do not over-engineer this before measuring.
- Damage is derived one walk later than wlroots derives it. No latency cost: the
  tick that would paint it is the tick that computes it.

### What still does not come from the diff

- [x] **Cursor.** Tick-owned and transactional today, computed in
  `tick_one_output` from `last_present_cursor_rect`/`version` (scene.rs:3265,
  3444) and retired through `PendingAck`. The HW-plane fast path moves the cursor
  with no scene damage at all (backend.rs:10278, 10348) and must keep doing so.
  Either the SW cursor becomes an ordinary participant in the diff — it is already
  an ordinary draw (scene.rs:4366-4375) — or it stays a separate producer. Decide
  explicitly; do not let it fall between.
- [ ] **Root `IncludeInferiors` XOR overlay.** Not a draw in the scene list at
  all — it is a separate logic-op pass (scene.rs:5716) — so the diff cannot see
  it. It injects explicit rect damage today (`root_overlay_toggle`, scene.rs:1282)
  precisely because `wake_for_damage` alone leaves the frame EmptyDamage-skipped.
  Unchanged by step 2.
- [ ] **The empty-projection force-compose** (scene.rs:3467-3565). Exists so
  snapshots whose projection landed empty can retire at all. Unchanged by step 2.

- [ ] **Content damage.** The largest producer, and it is not structural at all:
  `build_scene` peeks each drawable's presentation damage and projects it
  (scene.rs:4858, 5004). Sampled storage can change while the same `sample_view`
  stays bound, so the diff is blind to it by design. It keeps working exactly as
  today — but a reader who takes "damage falls out of scene changes" literally
  will look for it in the diff and not find it.

Four producers survive, not two: content damage, cursor, XOR overlay, and the
empty-projection force-compose. (`scene.bg_color` is a fifth in waiting — constant
black today, scene.rs:4084, and an empty draw list already forces Full to clear it,
scene.rs:3587. If it ever becomes real state it needs its own producer or a place
in the frame fingerprint.)

### Prerequisite: fix the coordinate space

- [ ] `mark_scene_structure_damage_rects` is fed screen-absolute rects — its only
  caller passes `window_absolute_rect` (backend.rs:5259-5284, explicitly "root is
  the (0,0) origin of the screen-absolute coordinate space") — but
  `dispatch_clip_rects_to_outputs` hands them to `clip_rect_to_output_extent`,
  which documents itself as taking **output-local** coords and omits the
  layout-origin translation on purpose (scene.rs:4977-4986). On a single output at
  origin (0,0) the two coincide, which is why this has never bitten; on a
  multi-output layout with a non-zero origin the damage lands on the wrong output
  or is clipped away entirely. `add_projected_damage` does the translation
  correctly (scene.rs:5004) — do the same here. Still worth fixing under the diff
  model, because `set_window_scene_participation` (backend.rs:17639) and
  `root_overlay_toggle` keep using this path.

### Which call sites change

- [x] *Demote to `wake_for_damage()`, let the diff derive the region:*
  `create_subwindow` (17216), `destroy_subwindow` (17244), `map_subwindow`
  (17286), `unmap_subwindow` (17300), `configure_subwindow` (17363),
  `reparent_subwindow` (17432), `register_top_level` (17519), `register_subwindow`
  (17543), `sync_top_level_order` (14802), the `set_container_background_*` family
  (18666-18780), `set_shape_rectangles` (23222). This is where the drag and resize
  win lives.
- [ ] *Add a `wake_for_damage()` they do not have:* the redirect alloc/release
  paths (17767, 17914), backing participation (17672), and storage realloc
  (3295-3339, 16958-17030). Under the diff these become ordinary damage sources
  for the first time — today they rely on an adjacent protocol call happening to
  wake the scene.
- [ ] *Leave coarse — lifecycle invalidations, not scene mutations:* degraded
  composed unflip (15222), `retire_direct_output` (2237), `finish_cow_release`
  (1631), `get_overlay_window` (18053). Each marks a transition in who owns the
  CRTC or the overlay, where "everything may have changed" is the honest
  statement and step 3.4's invalidation is the right primitive.
- [ ] `set_window_scene_participation` (17639) already posts rects and keeps
  doing so; it only needs the coordinate fix above.

### Gates

- [ ] The audit (per-BO candidates, 4.7) reports zero mismatches with a
  non-trivial `partial` count, through drag, resize and menu interaction — which
  is the **first time** the audit can say anything about window management at all,
  because those frames are `full` by construction today (scene.rs:541-546).
- [ ] `mean_damage` during drag and resize well below 1.0. That is the behavioural
  signal that the hammer is gone.
- [x] A test that a pure restack damages only the reordered participants —
  **tightened 2026-09-03 (`09f1c5d8`)**: only where they *overlap*. The first
  rule damaged the whole region of every participant whose rank index moved,
  which on the z400 e16 workload put mpv ∪ terminal (~0.64) on each raise/lower
  and forced Full frames. Now `⋃ (P ∩ Q)` over pairs whose relative order
  flipped, among rank-changed participants only. Finding:
  `../findings/2026-09-03-step1-z400-first-measurement.md`, last section.
- [ ] Hardware gate is the drag-shake question of 4.6, on non-composited MATE, on
  the hardware it was seen on. This is where that ghost has to be faced.

## Step 1 — visible regions, computed the way Xorg computes clip lists

**History, so the reasoning is not re-run.** 2026-09-02: `overdraw` measured 25×
on MATE — **a counter defect: it was recorded per walk against a per-compose
denominator, and the walk ran ~11.6× per compose in that run, so real overdraw
was ~2×** (found on the z400 2026-09-03, fixed the same day; see
`../findings/2026-09-03-step1-z400-first-measurement.md`). The conclusion
survived by luck: the scissor cull had already taken that saving on clipped
frames and Full frames were zero on awesome/MATE/e16 ⇒ step 1 demoted to its
correctness half. Same day, the reporter's run (#131, Polaris @4K) came in at **99% Full
frames** — a terminal tiled to ~all the screen repaints every frame, real damage
0.968, correctly over the clipping threshold — with `draws/compose = 6` and
6.73 ms per compose, most of it the full-output root painted under the
terminal. That reopens the GPU half for exactly his case: **Full-path
occlusion**. A naive whole-draw cull was tried and reverted; its failure is a
region-cap collapse, not a false premise — see
`../findings/2026-09-03-naive-occlusion-cull-postmortem.md`. Every premise it
rested on (draw index order is stacking order; `!alpha_passthrough` overwrites
its dst; per-output rects share one coordinate space) was verified from source
and **holds**. The e16 `±720` desk geometry was a startup transient of e16's
own desk slide and is not a gate.

### The model — X11 clip lists, in output-local integer coordinates

Xorg never paints a pixel twice for non-composited windows: `miComputeClips`
(`mi/mivaltree.c:197`) gives every window a `clipList` = its rect ∩ its parent's
clip − siblings above − its own children, and the union of all clip lists
partitions the screen. yserver keeps per-window storage and composes, so it
repaints overlaps; step 1 makes the compose emit each window only where Xorg
would let it own the framebuffer. Per node `n`, all in output-local pixels:

| term | definition | today's equivalent |
|---|---|---|
| `place(n)` | window rect ∩ every ancestor's rect ∩ own bounding shape, clipped to the output | what `presence_from_draws` unions — the current draw rects |
| `universe(n)` | the part of the output nothing above `n` has claimed when the walk reaches `n` | none |
| `visible(n)` | `place(n) ∩ universe(n)` | none |
| `opaque(n)` | `visible(n)` if `n` emits with `alpha_passthrough = false`; **empty** otherwise | `!alpha_passthrough` |

**Occlusion follows emission** (unchanged from the previous draft): a node
claims pixels only if it actually emitted an opaque draw. Manual-redirected
nodes (skipped) claim nothing; COW-subtree draws (`alpha_passthrough = true`)
claim nothing; the software cursor claims nothing; an Automatic-redirected
descendant of a Manual ancestor claims normally. This is Xorg's
`TreatAsTransparent(w) = (redirectDraw == RedirectDrawManual)` plus our blended
COW.

Draws become `visible(n)` **as rects**, each a `CompositeDraw`. A fully covered
window emits nothing; a partly covered one emits its visible pieces. The root
becomes `output − ⋃ opaque top-levels`, which for the reporter is a thin frame
around the terminal instead of 3840×2160 of overpaint.

**Coordinates (codex round 3).** `place` and `visible` are output-local and
**output-clipped** — that is new. Today the `intersects` gate only drops windows
entirely off the output; a straddling window keeps its negative or
past-the-edge dst and the rasteriser clips it (scene.rs ≈5091/5296). Once a
visible piece is output-clipped, its `src` can no longer be "clipped rect /
window size" in window-local terms copied from the shape path: derive it as
`(piece − window origin in output coords) / source extent`, i.e. translate the
output-local piece back into window-local pixels first. Get this wrong and a
window at negative x, or any window on an output with non-zero `layout_x0`,
samples the wrong part of its texture; the vertex shader interpolates
`src_origin + quad · src_size` verbatim (`composite.vert.glsl:39`). And the
denominator is the **sampled source's extent, not the host window size**: a
redirected window samples `redirected_target(id)`, whose backing may be larger
than the host geometry (`redirected_backing_can_fit` accepts `extent ≥ size`,
backend.rs ≈17852-17866), while today's UVs divide by `geom.width/height`
(scene.rs ≈5245/5278/5300). That is a latent stretch on any oversized backing
today; fix it in the factored decision rather than copy it. Pixel-oracle cases:
a window straddling the left edge, one on an output at `layout_x0 = 2560`, and a
redirected window with a backing 2× its host size.

### Order: compute top-down, emit bottom-up — one walk, reversed

Xorg computes clips with the topmost sibling first (each child gets
`universe ∩ borderSize`, recurses, then its `borderSize` is subtracted from the
universe) and paints bottom-up. A node's `visible` depends on its **children**
(they are above it) and on siblings **above** it, so it is known only after
those subtrees are processed. Emission order must stay painter's order — parent
before children, siblings bottom → top — because with a capped universe (below)
a lower draw may paint pixels a higher one also paints, and only order makes
that harmless.

The two orders are exact reverses of each other: computation order is
*children (top → bottom) then self*; reversing the emitted `Vec` yields *self,
then children (bottom → top)*, recursively — painter's pre-order. So:

- [ ] Build a **children index** once per `build_scene` (`HashMap<parent xid,
  Vec<(xid, rank)>>`, sorted). `emit_window_subtree` today scans the whole
  `WindowsMap` per node; the pass would double that, and the index removes it.
- [ ] Factor the per-node decision out of `emit_window_subtree` into one function
  returning `{ emits, opaque, place: Vec<Rect2D>, source view, participant id }`
  — the gate cascade (`mapped`, `is_manual_redirected`, `paint_target_is_self`,
  `kind == Window`, `source_view_null`, `intersects`) exists once, and the
  visibility pass cannot drift from the emission rules. The `place` rects are the
  current shape-path/unshaped-path clamps **plus the output clip**, in
  output-local coordinates (see "Coordinates" above for what that does to
  `src`).
  **`place` is geometry, not an emission result (codex round 2):** a node that
  does not emit — manual-redirected, no storage, `scene_participating=false` —
  still has a rect ∩ ancestors ∩ shape, and its descendants are clipped to it
  and claim through it. A manual-redirected top-level with an
  automatic-redirected child that emits opaquely must still subtract the
  child's area from the root's universe. So the decision carries `place` for
  every *mapped* node and `emits`/`opaque` separately; "does not emit" never
  means "has no place".
- [ ] **The root is a node in this walk**, not a draw pushed before it
  (today scene.rs ≈4544 emits the root first, then walks `top_level_order`).
  Its `place` is the output, its children are the top-levels, its `visible` is
  what they leave. Otherwise reversing the Vec puts the root on top of
  everything (codex). The SW cursor is appended **after** the reversal, so
  `software_cursor_tail` (scene.rs ≈981/4815) keeps pointing at the last
  draw/sample.
- [ ] Walk: `visit(n, universe: &mut Region)`, mirroring `miComputeClips`:
  1. `place(n)` from the node decision — **as an exact `Vec<Rect2D>`, never a
     `Region`**. The shape path already yields disjoint rects; unioning them
     into a `Region` would put them under the 32-box cap, and a collapsed
     `place` is a bounding box that *claims shape holes* (codex, 2026-09-03).
     That is a hole nothing fills, not overdraw. `place` rects stay exact.
  2. `mine = ⋃ᵣ (universe ∩ r)` over the place rects — a clone. Children are
     clipped by the parent's *rect ∩ shape*, so clipping them to `place(n)`
     (not its bbox) **is the parent-bounding-shape fix**, for free: Xorg's child
     universe is `∩ borderSize`, and `borderSize` is shape-clipped. `mine` can
     collapse (it is a union of intersections); when it does, children are
     clipped to the parent's *bbox* — exactly today's behaviour, no worse — and
     the collapse counter records it. Every other region in the walk is only
     ever intersected or subtracted.
  3. Recurse into children **top → bottom**, each `visit(child, &mut mine)`.
     Every opaque descendant subtracts itself from `mine` on the way out.
  4. `visible(n)` = for each place rect `r`: `mine ∩ r` — emitted **per place
     rect**, so an emitted piece never leaves its own shape rect even when
     `mine` is a superset. A superset here paints extra pixels *inside `r`*,
     which the children above overwrite: harmless under painter's order.
  5. Claim: if `opaque(n)`, `universe −= r` for each exact place rect (that is
     `visible(n)` plus everything the descendants took, since descendants lie
     inside `place(n)` — X11 clips them to the parent); otherwise the
     descendants' claims only, computed **per place rect without ever unioning
     `place`**: `for r in place { let mut taken = Region::from_rect(r);
     taken.subtract(&mine_after_children); universe.subtract(&taken); }`.
     `Region::from_rects(place)` is the obvious helper and the wrong one — it
     is the capped union again. If `mine` collapsed, `taken` under-claims
     (safe, costs occlusion). Subtraction of exact rects from a capped region
     can only yield a superset of the true remainder: the safe direction.
  Debug assertions: `universe_after ⊆ universe_before`; when `opaque(n)`,
  `visible(n) ∩ universe_after = ∅`.
- [ ] Reverse `draws` and `participants` at the end of the walk (before the
  cursor append). Presences are built from `place` at the node's own step (see
  the step-2 paragraph below — `presence_from_draws` is retired), so a node's
  presence is pushed at the same point as its own draws and the two lists
  reverse consistently. `snapshots` / `sampled_ids` are sets; order irrelevant.
- [ ] Top-levels take `universe = output extent` (a real region now, replacing
  the `i32::MIN/2..MAX/2` sentinel clip). `suppress_cow` stays exactly as is and
  still skips the COW top-level.

The alternative — a separate top-down visibility pass producing a per-xid
`visible` table read by the unchanged bottom-up emitter — walks the tree twice
and duplicates the gate cascade. Take it only if reversing turns out to
complicate something the walk threads (nothing found so far).

### The cap, and the one safe direction

`Region` collapses to its bounding box above 32 boxes. In this design **the
universe is only ever subtracted from and intersected**, so a collapse yields a
*superset* of the true unclaimed area: nodes below emit more than needed
(overdraw, harmless under painter's order), never less (a hole). Nothing in
the pass may build a covered-union and test containment against it — that is
the reverted cull's defect, restated as a rule. The two places a union does
appear are bounded by construction: `place(n)` is never a `Region` (exact rects,
step 1 of the walk), and `mine` (step 2) may collapse only into "children
clipped to the parent's bbox", which is today's behaviour. Audit every
`Region::add_rect` / `union_with` in the pass against this list before landing.

- [ ] `telemetry`: count universe collapses per compose (`visibility_collapses`)
  and draws emitted vs. nodes visited. A scene that collapses every frame is one
  where the pass is buying nothing; e16 is the candidate.

### Step 4's opaque-cover gate must change in the same commit

`opaque_cover_exists` asks for a **single** opaque draw containing each scissor
rect. Once the root is `output − windows`, no single draw contains a repaint rect
that straddles a window edge; the gate would fail on most frames and send them
Full, and the step-4 win would silently vanish while the screen looked perfect.

- [ ] Replace with a remainder test: `remainder = scissor rect; for each opaque
  draw: remainder −= inward dst; covered ⇔ remainder.is_empty()`. Subtraction
  from a remainder is the safe direction (a collapsed remainder is a superset ⇒
  "not covered" ⇒ Full). Run it per scissor rect, on the draws that intersect
  that rect.
- [ ] **Ordering (codex):** today `plan_repaint` gates on `built.scene.draws`
  and `cull_scene_to_region` runs *after* planning (scene.rs ≈3801-3812), and
  the post-cull `debug_assert!(opaque_cover_exists(&c.draws, rect))` at ≈3817
  checks ONE draw against the repaint **bbox**, not each scissor. Both must
  change together: gate on the intersecting draws per scissor inside planning
  (or cull first and plan on the culled list — it is a separate product either
  way), and make the assertion the same remainder test per scissor rect, or it
  fires falsely on every 4.5 multi-rect frame over a fragmented root.
- [ ] **Descriptor completeness (codex):** `record_command_buffer` draws only
  the allocated descriptor prefix (scene.rs ≈5938 `break`, ≈6171 `.take(...)`),
  and only the audit checks `compose_submit_was_complete` (≈3014); the
  production tick stages `painted` regardless. A cover proof over draws that
  were never recorded is void. On the clipped path an incomplete submit must
  **not** stage `painted` — `damage.invalidate()` instead. Cheap, and this step
  is the one that changes draw counts (fewer overall, but up to 32 pieces per
  partly covered window).
- [ ] Test: root fragmented around a window + the window ⇒ a rect across the
  edge is covered; the same with one uncovered pixel row ⇒ `NoOpaqueCover`;
  two scissors each covered by *different* draws ⇒ Clipped.

### Step 2's diff must keep reading placement, not visibility

`ScenePresence::region` feeds `structural_damage`, which damages `old ∪ new` for
any participant whose region changed. If that region became `visible(n)`, every
window uncovered by a move above it would read as "moved" and damage its whole
footprint — on every restack of anything, for everything beneath. wlroots
damages the *mover's* old ∪ new only; the uncovered nodes are repainted by that
damage because they lie inside it.

- [ ] `ScenePresence.region` stays `place(n)` (unclipped placement). Derive it
  from the node decision's `place` rects, not from the emitted (clipped) draws.
  Concretely: the two `presence_from_draws` call sites (root, scene.rs ≈4577;
  window, ≈5319) go away, replaced by a `presence_from_place(place, id,
  signature)` — left as they are, the diff would read every visibility change
  as a placement change and the drag/restack damage would balloon (codex).
  **The signature must not come from an emitted piece either.**
  `PresenceSignature` holds `src_origin`/`src_size`; once draws are visible
  pieces, the first piece's src moves whenever the cover above moves, and the
  diff would report `resampled` — old ∪ new placement — on every such frame.
  Derive the signature's src from the first *place* rect (what the unclipped
  draw would carry) and the view from the node decision. A redirect swap or
  storage realloc still changes the view, so it still damages.
- [ ] **A fully hidden participant is still a participant (codex round 2).** If
  a window covered by an unrelated move above it emits nothing and drops out of
  `participants`, `structural_damage` (scene_diff.rs ≈242) reads it as vanished
  and damages its whole placement, then again as appeared when uncovered —
  exactly the hammer step 2 removed. Record it with `region = place`,
  `visible = ∅`, the same signature, zero draws. Rank among common participants
  is then stable too.
- [ ] Add `ScenePresence.visible: Region` for the content-damage clip below. It
  is **not** part of the diff's equality.
- [ ] Test: a window uncovered by an unrelated move above it contributes no
  structural damage of its own; the mover's damage covers the uncovered area
  (pixel oracle, see below).

### Content damage clipped to `visible` — and the trap beside it

`add_projected_damage` (scene.rs) projects storage damage onto the output with no
visibility clip, so a mostly-covered window repainting continuously damages
screen area that cannot have changed. That is the leak the plan has named since
round 6.

- [ ] Intersect each projected rect with `visible(n)` before adding.
- [ ] **The empty-projection force-compose is now a trap — resolved by NOT
  acking (stage C, 2026-09-03).** The tick's empty-damage path (scene.rs,
  "xfce submenu painted but not shown") forces a **Full** compose whenever a
  snapshot carries non-empty captured damage but its projection landed empty,
  because until step 1 that only meant a geometry gap. After visibility
  clipping it also means "the window painted where it is covered" — the common
  case for a window under a terminal — and forcing a Full compose per hidden
  paint would invert the whole goal. Stage C classifies each snapshot per
  output: **OffOutput** (unclipped projection empty — keep today's force),
  **Hidden** (unclipped projection non-empty, clipped projection empty — do NOT
  force), **Visible**. Hidden damage is **not acked**: an un-acked snapshot is
  simply re-peeked next walk; the damage accumulates in the store (capped,
  superset, safe) while the window stays covered, and when it is uncovered the
  mover's structural damage repaints the area and the accumulated damage
  projects visibly, composes and acks like any other. This also dissolves the
  multi-output hazard codex raised in rounds 1-2 (`ack_presentation_damage` is
  global per drawable): a window hidden on A and visible on B is composed and
  acked by B, and A never touches it. Cost: each paint into a hidden window
  still wakes the tick and walks — the wake-rate item, out of scope. The
  earlier proposal to ack hidden snapshots after every output walked, gated on
  "no output captured the drawable into a live PendingAck", is superseded and
  kept only in git history.
- [ ] Test: paint inside the covered part of a window ⇒ tick skips with
  `EmptyDamage`, snapshot acked, no compose. Paint in the visible part ⇒ damage
  = that part only.

### The parent-bounding-shape bug and `suppress_cow`

- [ ] Write the missing test first: an empty parent `shape_bounding` suppresses
  its children; a partial parent shape clips them. With `child_universe =
  universe ∩ place(n)` it passes by construction. Record it in the module doc
  (and fix the stale doc at scene.rs:26 while there — the manual-redirect prune
  it describes was removed by audit #3).
- [ ] **Do not retire `suppress_cow` here.** It is not an occlusion the tree can
  express: the fullscreen window is *below* the COW in stacking and occludes it
  only because the compositor unredirected it. Keep the probe and its filter
  matrix (`topmost_on_output` skips the COW, requires `mapped` and an
  intersection with this output — muffin's off-screen 1×1 helpers, #98; then
  covers ∧ `depth != 32` ∧ `scene_participating`). Pin each with a test before
  anyone touches it; failure mode is a fullscreen game rendering as wallpaper.

### Tests — a pixel oracle, not per-case assertions

The invariant is "the clipped scene shows the same pixels as the unclipped
one". Test it directly:

- [ ] Rasterise both draw lists on a small integer grid: for each draw in order,
  for each pixel in its dst, push `(sample_view, src pixel)`; an opaque draw
  resets the pixel's stack. Compare stacks. Runs against synthetic `WindowsMap`
  trees: nesting, siblings overlapping, shaped parents, a COW subtree above
  opaque windows (must not claim), a manual-redirected node with an emitting
  automatic child, windows straddling the output edge, a non-zero layout origin,
  and a >32-fragment universe (collapse ⇒ over-emission, oracle still equal).
- [ ] Assert the reverse-order property: emitted draws are in painter's
  pre-order over the same tree the old walk produced (compare against the
  pre-step-1 emitter on trees with no occlusion, where both must be identical).

### Cost — this is the step that can trade GPU for CPU

The pass adds Region ops per node. Region operands are capped at 32 boxes, but
`place` is exact and uncapped, so the work is **per place fragment**, not per
node: roughly `fragments × 3 × O(32)` for the `mine`/`visible`/claim steps, and
up to `O(32·32)` per fragment on the non-opaque `taken` path (codex round 3).
On e16 that is ~2265 fragments ⇒ thousands of region ops per output per frame;
on MATE ~56. It runs on the thread that also handles protocol, on a 2009 Xeon.
`avg_compose_cb_record_ns` starts **after** `build_scene`, so the walk is
unmeasured today.

- [x] `avg_build_scene_ns` / `build_scene_calls/s` landed with stage A
  (`2e2d721f`). **Measured 2026-09-03 on silence, before the walk gets any
  heavier:**

  | | MATE phased workload | e16 hand-driven |
  |---|---|---|
  | walk per call, median | 39 µs | 312 µs (max 755) |
  | walk calls/s, median | 578 (max 5836) | 436 (max 1070) |
  | composes/s | ~50 | ~50-100 (2 outputs) |
  | **walk ms per second** | **23 (2.3% of a core)** | **140 (14%, max 28%)** |
  | `cb_record` ms per second | 6.6 | 7.8 |

  Two things this settles. **The walk already costs more than command-buffer
  recording**, 3-18×, because **it runs on every wake, not per compose** — the
  tick cannot know whether damage is empty until the walk has projected the
  presentation-damage snapshots, so ~11 walks per compose on MATE is the
  design, not a bug. Any per-fragment cost stage B adds is multiplied by that
  wake rate, so the cost gate is `walk ms/s`, not per-call µs. And on e16 the
  walk is already a real CPU consumer (up to 28% of the render thread) with
  zero occlusion work in it — the per-node WindowsMap scan is gone since stage
  A, so what remains is 2265-3000 nodes of decision + logging + draw building.
  The hand-driven e16 run is not comparable to a phased one; rerun
  `just yserver-e16-hw-workload` after stage B for the before/after.
- [ ] Measure before/after on the z400 (target) as well. Added CPU shows up as
  input latency, which GPU-only acceptance misses.
- [ ] **Stage B lever, if the walk gets expensive:** the wake rate. A tick that
  walks only to discover empty damage could be skipped when no presentation
  damage, structure change or cursor move is pending — that is a tick-driver
  change, out of step 1's scope, but it is where a 5-10× saving lives on e16.
- [ ] If the pass costs more than it removes on the dense scene, skip
  visibility derivation on frames the plan already knows will render Full for a
  reason other than overdraw (`EmptyDrawList`, `UnloadableBo`, `CopiedRoute`),
  or collapse the universe earlier than 32 for the walk only. Both are safe
  directions.

### Gates

- Damage audit (`YSERVER_DAMAGE_AUDIT=1`) clean on awesome, MATE, e16 — **with
  the reference rendered from an UNCLIPPED scene.** Today candidate and
  reference both render `scene` (scene.rs ≈2867/2897), so a visibility bug
  that removes pixels from both passes clean; the audit would be vacuous for
  this step (codex round 2). Produce the unclipped list audit-only by running
  the factored node decision with visibility disabled (`universe` = the whole
  output for every node, no subtraction) — one extra walk per audited frame,
  behind the env gate. Then the audit is a real regression test for step 1 on
  hardware, and the synthetic pixel oracle covers what the audit cannot
  (trees the WMs do not build).
- Pre-existing, noted not fixed: `shape_bounding` stores the client's
  ShapeRectangles as given; if they overlap, a shaped alpha window already
  double-blends per overlapping piece, and per-place-rect emission inherits
  that. Canonicalising shape rects at SHAPE-set time is a separate change
  (it must not go through the capped `Region`).
- `just yserver-{awesome,mate,e16}-hw-workload`: Full frames still zero,
  `painted` fractions ≤ today's, `avg_compose_cb_record_ns` + `build_scene_ns`
  within noise on MATE and reported for e16.
- Reporter's case reproduced locally (terminal tiled to the screen, `mpv` in
  it): Full-path `overdraw` → ≈1.0 and `avg_gpu_render_ns` on 100%-Full
  buckets down; then his rerun on a build ≥ this step. Expectation for his 6.73
  ms is "≈ the terminal's own cost", not a number — his build had no `overdraw`
  counter, so the size is unmeasured until he runs one that does.
- Eyes-on: e16 pager, wallpaper and drag-bar; MATE panels; a fullscreen
  unredirected window under Cinnamon (`suppress_cow` path).

## Verified clean — do not re-investigate

Four rounds of review checked these and found nothing. Recorded so the next
reader spends their scepticism elsewhere.

- **Narrowing `render_area` to the clipped rect is Vulkan-consistent.** The
  layout barriers are full-subresource (scene.rs:5600-5625, 5120-5139) while
  dynamic rendering is free to use a smaller `render_area` (scene.rs:5637-5642);
  `AuditClearClipped` already does exactly this (scene.rs:5561-5563), and the
  timestamp bracketing is unaffected (scene.rs:5580-5586, 5724-5728).
- **The dead `Repaint::Clipped` LOAD path's `old_layout = GENERAL` assumption
  holds** — given the unloadable-BO gate of 4.2. Shared BOs are only acquired in
  `BoPhase::Free` (platform.rs:5054-5060), a successful compose leaves them in
  GENERAL via `record_post_compose` (scene.rs:5120-5127), page-flip retirement
  returns the prior on-screen BO to free (platform.rs:6193-6205), and failed
  submits invalidate content (scene.rs:3831-3836, platform.rs:5220-5226). So the
  plan is not switching on code that was never right.
- **Multi-output asymmetry is safe.** One output rendering Full while another
  renders Clipped on the same tick breaks nothing: `tick` folds only the global
  dirty flag and all-output walk state (scene.rs:1622-1657), `TickOutcome`
  preserves dirty on the skip paths (scene.rs:831-851), `offscreen_no_draw`
  reconciliation waits for every output (scene.rs:1653-1654), and the overlay
  apply list is already per-output (root_overlay.rs:85-100).
- **4.6's drag argument holds for step 4.** Configure-driven drag reaches
  `mark_scene_structure_dirty` (backend.rs:17363) which posts full-output rects to
  every output (scene.rs:1202-1210), so the painted-area threshold forces Full.
  This says nothing about step 2, where drag becomes partial — that caveat is the
  point of 4.6.

## What the phased workload can and cannot resolve

Two runs of `tools/damage-workload.sh` on identical scripted input, minutes
apart, on binaries differing only by 2c — which is inert on a single output at
the origin. So this is a pure repeatability measurement
(`../findings/data/2026-09-02-step2c-workload-silence-*.log`):

| metric | behaviour across identical runs |
|---|---|
| `damage_fraction`, `damage_region_fraction`, `structural_fraction` | **identical to three decimals** in all six phases |
| `full_redraw_fallback/s`, `clipped_repaint/s` | identical |
| `avg_gpu_render_ns` | **±6-26%, no pattern** |

So on this box:

- **The area counters are deterministic.** Use them to confirm an effect. 4.5's
  drag improvement was accepted because `painted` fell to exactly the requested
  region — a deterministic quantity — not because the GPU number moved.
- **`avg_gpu_render_ns` has a ±25% noise floor.** Only deltas well clear of that
  mean anything. The 55% step-4 halving and the 36% 4.5 drag win clear it; a 20%
  effect does not, which is why the step-2 magnitude was abandoned on silence
  rather than chased.

State the effect size a run can resolve *before* running it. Both inconclusive
measurements on 2026-09-02 came from not doing that.

## Traps, carried forward

Restated here because each has already cost this campaign time.

- **`damage_fraction` in production telemetry is 1.000 by construction** until
  task 4.4 lands. It measures nothing before then.
- **The audit's absolute µs do not transfer to production.** It composes into a
  private OPTIMAL-tiled image; production composes into the LINEAR scanout BO.
  Only the ratio transfers.
- **LINEAR is not the cause and not the fix.** Xorg and labwc do this workload on
  the same card into the same buffers for nearly nothing. Do not go chasing
  modifiers.
- **Measure on hardware that reproduces.** The z400 + RX 460, not silence or bee.
  Fast-GPU numbers produced a "weak perf case" conclusion that was simply wrong.
- **An audit run whose comparisons are all `full` or `idle` is not evidence.**
  Check the `partial` count before believing a clean run.
- **`grep heartbeat` first on any audit run.** No heartbeat means it never armed;
  and the `YSERVER_*` var belongs on the yserver line of the `just` recipe, not
  the WM line, where it silently does nothing and the log looks clean.
- **Damage snapshots are not proof the pixels landed.** The store's ack is
  region/epoch based (store.rs:1150-1178) while the compose only touches render
  fences after a successful submit (scene.rs:3795-3798). The audit's unexplained
  frame-2 divergence lives in this gap — a compose sampling storage whose paint
  has not landed. Always-Full hides it (the next frame repaints everything);
  clipped repaint does not, because the region is then marked clean. The
  boot-flush prerequisite is necessary but may not be sufficient; if step 4's
  hardware smoke shows one-frame stale content that heals on the next unrelated
  damage, this is the first place to look, not the region algebra.
- **Proxies that must not be mistaken for the measure:** `mean_damage`, µs per
  compose, partial-comparison counts, frame time. The measure is GPU utilisation
  on the z400 against labwc.

## Toolchain

Per AGENTS.md, before every commit: `cargo +nightly fmt`, `cargo clippy --all-targets
-- -D warnings` (exactly as CI; regular clippy, not pedantic), `cargo test`. No
commit before hardware smoke on anything that touches what reaches the screen.
