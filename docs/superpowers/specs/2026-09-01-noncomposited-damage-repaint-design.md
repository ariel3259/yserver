# Non-composited damage repaint — design

> **SUPERSEDED** by `2026-09-01-damage-derived-scene-repaint-design.md`.
>
> The canonical-scene-image design below was anchored on Xorg modesetting
> TearFree — the right reference for protocol, the wrong one for render
> architecture. wlroots is in yserver's architecture and needs no authoritative
> image. Kept for §Prior attempts, §Invariants and the Phase 0 audit, all of
> which remain valid; see the new spec for the one correction to the invariants.

## Status

Design only. Nothing here is implemented.

This supersedes the `fix/clipped-noncomposited-repaint` branch, which is
abandoned. That branch's canonical-scene series was reverted commit-by-commit
and its net effect on `master` was negative (see §Prior attempts). Two useful
things came out of it and are already on this branch: the `RegionSet::subtract`
multiset fix (`355c221f`, cherry-picked) and the knowledge written down below.

Read §Prior attempts and §Invariants before proposing anything. Three attempts
have failed in the same way.

Reviewed by codex 2026-09-01, three rounds, all requested changes applied.
Round 1: Phase 0 provenance instrumentation, Shared-only scope, the split cursor
invariant, "qualified" rather than "proved" as the Phase 0 exit. Round 2: the
persistent Phase 0 candidate image, the transition ledger, and the split
root-overlay invariant. Round 3: the split empty-draw-list invariant, and
provenance keyed by ledger event ID rather than call site.

A theme ran through all three rounds and is worth stating once, because it will
recur: **every invariant on `master` was written against direct-to-BO buffer-age
repaint, and none of them transfers unexamined to the canonical design.** Three
of the four (cursor, root overlay, empty draw list) turned out to be strictly
weaker under a canonical image, because per-BO fan-out or the
clear-within-repaint-rect rule subsumes the retained-BO argument they rest on.
Before carrying any remaining `master` hazard note into Phase 2, check which
design its reasoning assumes — but weaken one only with the argument written
down, never by deletion.

## Goal

Stop rasterising the whole desktop on every frame in the non-composited case,
without reintroducing stale pixels.

Scope is the KMS compose path only. Window bit-gravity / resize pixel
preservation is a **separate issue on a separate branch** — it was bundled into
the abandoned branch and that bundling is part of why the branch became
unreadable.

## Current behaviour on master (`ad318afa`)

`tick_one_output` composes the full scene into the acquired scanout BO with
`loadOp=CLEAR` every frame. `pick_repaint_region`
(`crates/yserver/src/kms/render/scene.rs`) still exists but returns
`Repaint::Full` unconditionally; the buffer-age logic below it is commented out.
`output_damage` is still computed, and drives the empty-damage tick skip, the
telemetry, and the buffer-age history ring — but not the repaint region.

Consequence: correct, and `full_redraw_fallback/s == frame_present_count/s`.

## Prior attempts

1. `b874d507` buffer-age clipped repaint → reverted `3c75e74e`. Visible
   multi-pixel "drag-shake" on non-composited MATE: stale drag-phase content
   left in BOs the catch-up scissor did not cover.
2. `perf/reenable-buffer-age` (`f52796d1`, env-gated, never merged) → shelved on
   air/Asahi. **Background/stale content peeked through even a completely static
   window.** History recording was correct; the gap was damage completeness.
3. `fix/clipped-noncomposited-repaint`, the canonical-scene series
   (`22005d21` → `a039b152` → `bf8e6950`) → `bf8e6950` reverted within the hour
   by `7ca33731`. It shipped a 32-pixel `inflate_damage_for_repaint` guard band.
   That guard band is the tell: it makes the artefact rarer, never absent, and
   has no correct value.

The common cause is not BO recycling. It is that `output_damage` does not
describe every pixel whose composited value changed.

## Two independent problems

**P1 — BO recency.** The pool holds N scanout BOs, each last written at a
different generation. A partial repaint into a recycled BO must also repaint
everything that changed since *that BO* was last presented. Solvable by
buffer-age history, or by compositing into a persistent canonical image and
copying per-BO pending damage into each BO.

**P2 — damage completeness.** The damage set does not cover every pixel whose
final composited value differs from the previous frame's.

Neither buffer-age nor a canonical image touches P2. Every previous attempt
solved (or re-solved) P1 and then enabled clipping in the hope that P2 was fine.
It is not. A canonical image makes P2 strictly *worse* in one respect: because
the canonical persists and is never wholly recomposed, an under-reported region
latches indefinitely instead of self-healing on the next frame.

## P2 is the gate, and it is mechanical

Two damage-marking APIs feed `output.scene_structure_damage`:

- `SceneCompositor::mark_scene_structure_dirty()` (scene.rs:835) adds a
  **full-output rect** on every output. ~20 call sites in
  `kms/render/backend.rs`: map, unmap, configure, restack, redirect,
  root-background, and friends. Coarse but complete.
- `SceneCompositor::wake_for_damage()` (scene.rs:827) sets a bool and
  **contributes no region at all**, deliberately: its doc comment asserts that
  "protocol paint is already represented by per-drawable presentation damage,
  and cursor motion is projected by `build_scene`." 22 call sites (18 in
  backend.rs, 3 in scene.rs, 1 in platform.rs).

Under `Repaint::Full` that asymmetry is invisible — every wake repaints
everything. Under clipped repaint, each `wake_for_damage()` site is a candidate
hole: it is only safe if that asserted invariant actually holds there.

The other producer is `built.projected_damage` — per-drawable presentation
damage projected into output coordinates (`add_projected_damage`, scene.rs).
That describes where a *client painted*, not where the *composited result*
changed. Known divergences: a window that moved (nothing damages the vacated
position unless a structure-damage call covers it), unmap, restack, shape
change, translucent stacking, border, and cursor.

So P2 is not mysterious. It is a countable audit list: 22 `wake_for_damage()`
sites plus the projection gaps. That list should be produced by measurement, not
by reading.

### The corollary nobody has priced in

`mark_scene_structure_dirty()` damages the **whole output**. It is unioned into
`output_damage` (scene.rs:2271), so the repaint bounding box becomes full-screen.
That means: even with P2 fully solved, clipped repaint yields **nothing** during
window management — every map, unmap, configure, restack and redirect change is
a full-screen repaint by construction.

The win is therefore confined to steady-state per-window paint: mpv, a terminal,
an idle awesome desktop. That is still worth having, and it is the workload in
the acceptance criteria — but the project should not be sold as "menus and
window drags get cheaper", because with today's structure-damage granularity
they do not. Making those cheaper is a separate, larger piece of work
(rect-precise structure damage; only `mark_scene_structure_damage_rect` /
`_rects` do that today, and there is exactly one backend caller,
backend.rs:17614).

## Phase 0 — measure. This decides whether the project proceeds.

Deliverable: a diag branch. Not a merge candidate.

- Leave production on `Repaint::Full`. That full compose is the **reference**.
- Maintain a second, **persistent candidate image** per output, and compare its
  *entire extent* against the reference every frame.
- Exercise: non-composited MATE (menus, drag, resize), awesome, i3 with a
  floating-window drag, mpv, and a **static desktop soak** — the case that
  shelved attempt 2.

### The candidate image must be persistent, and that is the whole design

A per-frame scratch image cannot measure anything. Left uninitialized its pixels
outside the damage region are undefined, so everything mismatches. Seeded from
the current full frame it imports the correct answer, so nothing ever mismatches
— it would hide precisely the bug being hunted. Either way the diagnostic is
worthless, and both are easy mistakes to make.

So:

- Initialize the candidate with **one** full composition.
- On every subsequent frame, update **only** `output_damage` within it, using
  the same clear-within-repaint-rect rule Phase 2 will use (`loadOp=CLEAR` with
  `renderArea` = the repaint rect). Phase 0 must isolate P2; if the candidate is
  updated with `loadOp=LOAD` it will also reproduce the missing-background defect
  described in Phase 2 and report it as a damage-completeness failure.
- Diff the candidate against the reference over the full output extent.
- Reset — re-initialize with a full composition, and suppress mismatch counting
  for that frame — on: modeset / RANDR resize, VT-switch and DPMS resume,
  `DEVICE_LOST`, failed submit, `invalidate_bo`, and any partial frame (the
  descriptor-allocation `break` in `record_and_submit_render`). These are the
  same events Phase 2 lists as forced-full-recompose triggers; if Phase 0 does
  not reset on them it will report their artefacts as damage bugs.

The payoff is that this makes Phase 0 a faithful simulation of Phase 2 rather
than an approximation of it. The candidate retains errors the way the canonical
image would, so a missed damage event does not merely flicker past in one frame.
That is what makes the static-desktop soak a sharp test rather than a vague one:
with nothing else painting, a hole has nothing to hide behind.

Retained is not the same as permanent, and the distinction matters for the
harness. Later legitimate damage covering the same area will repaint it and heal
the divergence — under a busy workload that can happen within a frame or two. So
the comparison must be **edge-triggered**: report every `matched → mismatched`
transition with its own start frame and clear the state when a tile heals, never
a write-once record per tile, which would mask a second independent failure in
the same place. It also means a *sampled* comparison can step straight over a
real failure, which is why the qualifying runs compare every frame.

A healed divergence is still a damage hole. It got covered by unrelated later
damage, which is luck, not correctness, and a different workload will not be so
lucky. Count and report those episodes rather than discarding them.

### Provenance instrumentation is part of the deliverable, not an extra

A mismatching rect plus the overlapping draw xids says *what* is wrong. It does
not say *which* producer failed to report, and that is the only output that
turns P2 into fixable work. Without it Phase 0 reproduces the failure without
explaining it — which is what the previous three attempts already achieved.

Two records are needed, and only having the first is the trap.

**(a) Contribution provenance, keyed by event identity — not by call site.**
Every `RegionSet` contribution carries a **frame-local ledger event ID** naming
the transition it came from, not merely the name of the function that added it.
A call site alone misattributes: two windows configured in the same frame go
through the same `mark_scene_structure_dirty()` site, and "this rect came from
the configure site" cannot distinguish them, which is the case that matters when
one of the two reported and the other did not.

Note what this rules out: `wake_for_damage()` adds no rect, so it can carry no
contribution provenance at all. It is **not** a producer for the purposes of
(a) — listing its 22 sites here would be incoherent. Those sites appear only in
the ledger, as transitions that are expected to be covered by per-drawable
presentation damage. Whether that expectation holds is precisely the question
Phase 0 exists to answer.

**(b) A per-frame transition ledger.** Annotated contributions can only describe
damage that *exists*. The interesting failure is a transition that contributed
*nothing*, and no amount of annotation on the empty set will surface it.
Correlating a mismatching rect with whichever producers happened to contribute
nearby is correlation, not attribution, and will send the reader after innocent
call sites.

So record, independently of and prior to damage calculation, a ledger of every
scene-affecting transition in the frame. Each entry gets a frame-local **event
ID** and holds: the call site, the drawable, old/new state — geometry, shape
serial, stacking position, source version, cursor position and version,
map/redirect state — and an **expected affected area**. The ledger is the
authority on what happened; (a)'s contributions merely point back into it by ID.

Attribution is then a join on that ID, not a guess from geometry: for a
mismatching rect, which ledger entries' expected areas cover it, and did *those
specific events* — identified by ID, not by the function they went through —
contribute any damage at all? An event whose expected area covers the mismatch
and which produced no contribution is the answer. The `wake_for_damage()` sites
will be the interesting ones, since by construction they never produce a
contribution and their entire safety argument is that per-drawable presentation
damage covers them.

The obvious objection — "if you can compute the expected area, use it as the
damage" — is what makes this work rather than what breaks it. The ledger's
expected area may be deliberately dumb, over-broad and slow: whole old bounding
box ∪ whole new bounding box, no clipping, no occlusion reasoning, recomputed
from scratch. It never ships. Production damage has to be tight and cheap, which
is the entire reason it has holes. The diag is allowed to be neither.

**Attribution must point at the frame divergence began, not the current frame.**
Because the candidate image persists (see above), a mismatching region survives
every subsequent frame. Record, for each mismatching region, the first frame at
which it appeared, and correlate against *that* frame's ledger. Correlating
against the current frame's ledger will attribute a long-latched error to
whatever unrelated thing happened most recently.

This is diag-branch-only instrumentation and is expected to be thrown away; it
does not need to be cheap and it must not land on `master`.

Exit criteria:

- Zero mismatch across every exercised workload for a sustained run ⇒ P2 is
  **qualified** for those workloads and that duration. This is a gate, not a
  proof — the state space is not covered by six hand-driven sessions. Where more
  confidence is wanted, add randomized scene mutation (map/unmap/configure/
  restack/shape/stack-order under a seeded generator, replayable from the seed)
  or a deterministic model test that recomputes expected coverage from the scene
  graph and compares it against the reported damage. Prefer the model test: it
  runs in CI and does not need hardware.
- A bounded, attributable list of producing sites ⇒ fix those sites, re-measure,
  then Phase 1.
- Unbounded or unattributable mismatch ⇒ **the project stops here**, and we
  record that rather than building scaffolding for a payoff that cannot land.

Rationale: all three prior attempts enabled clipping and judged the result by
eye. Eyeballing cannot attribute a stale rect to the producer that failed to
report it, which is why each attempt ended in either a revert or a guard band.
If the Phase 0 output tempts anyone to inflate the damage region, that is the
signal that a producer is missing — not that the padding needs tuning.

## Phase 1 — canonical scene image (only if Phase 0 passes)

### Reference behaviour, verified in `../xserver`

Xorg modesetting's TearFree path:

- `ms_tearfree_update_damages` (`hw/xfree86/drivers/modesetting/driver.c:667`)
  intersects the screen DAMAGE region with each `crtc->bounds` and unions the
  result into **every** TearFree buffer's `dmg`, then `DamageEmpty(ms->damage)`.
- `ms_do_tearfree_flip` (`.../pageflip.c:658`) calls
  `drmmode_copy_damage(crtc, trf->buf[idx].px, &trf->buf[idx].dmg, TRUE)`, which
  copies that buffer's pending region out of the screen pixmap and **empties the
  region at copy time**, then queues the flip.
- On flip failure it copies into the front buffer *without* emptying and renders
  untorn for that frame.
- Exactly two buffers. The screen pixmap is only ever a copy *source*.

### Where the analogy stops — state this loudly

**Xorg never rebuilds the screen pixmap from a damage region.** Clients draw
into it directly through the clip/validate machinery, so it is authoritative by
construction and its DAMAGE region is complete by construction.

yserver has no such pixmap. It recomposites a scene from per-window storage
every frame. So TearFree justifies the **BO fan-out** (P1) and says **nothing**
about P2. The real precedent for recompose-from-damage is the Wayland
scene-graph compositors (wlroots `wlr_scene`, weston), which carry per-node
visibility bookkeeping that we do not have. Citing TearFree as authority for the
clipped-recompose half — as the abandoned branch's spec did — is the error that
made P2 look already-solved.

### Applies to `OutputScanout::Shared` only

Phases 1 and 2 apply **only to `OutputScanout::Shared` outputs**.
`OutputScanout::Copied` — the reverse-PRIME path — keeps its current
direct full-render path, unchanged and un-canonicalised.

This is a deliberate choice, not an oversight, and it has a cost worth naming:
the compose recorder then carries two target paths instead of one. The
abandoned branch avoided that fork by routing Copied through the canonical too
(that is the sole reason `record_transport_copy_from_layout` was added to
`kms/vk/scanout.rs`). Routing both is the tidier code and the worse deal: a
Copied output would pay a permanent extra full-screen blit while being excluded
from clipped repaint, so it gets pure cost and no benefit. It also widens the
blast radius onto a path that is hard to HW-test and already has unresolved
problems of its own (PR #95). Keep the fork; keep Copied out.

Any implementation that finds itself adding a canonical layout parameter to a
`CopiedRenderSource` method has drifted off this design.

### Design (Shared outputs)

- One device-local `B8G8R8A8_UNORM` image per Shared output (every scanout BO in
  the tree is already `B8G8R8A8_UNORM`, so the copy is format-exact), extent =
  the output extent, usage `COLOR_ATTACHMENT | TRANSFER_SRC`. Torn down and
  recreated with the output on modeset / RANDR resize.
- The scene composes into it. For a Shared output it is the only render target.
- Per-BO pending damage region. Every canonical update unions into **every**
  BO's region.
- At flip time, copy that BO's pending rects out of the canonical, then retire
  its region.

### One deliberate deviation from Xorg, and why

Xorg empties the region at copy time because `drmmode_copy_damage` ends in
`glamor.finish()` — the copy has completed before the flip is queued.

yserver records the copy into a command buffer that executes asynchronously and
retires on the pageflip event. Damage can therefore arrive between submit and
retire. The region must be **snapshotted at submit and subtracted at retire**,
not cleared. This is exactly the bug `355c221f` fixed for
`scene_structure_damage`; the existing pattern to follow is
`state.scene_structure_damage.subtract(&ack.submitted_scene_structure_damage)`
(scene.rs:1331), and `RegionSet::subtract` is multiset subtraction precisely so
a repeated identical rect survives.

### Honest cost accounting

Phase 1 **on its own is a pure regression**: an extra full-screen device-local
image per output, plus a full-screen blit every frame, producing pixels
identical to today's — because `Repaint::Full` still forces a whole-canonical
copy. It is scaffolding for Phase 2 and must not be merged alone. The abandoned
branch shipped exactly this state and left it there.

## Phase 2 — clip the canonical recompose

- **`loadOp=LOAD` alone is wrong.** It never re-establishes the root/background
  base inside the repaint region, so the pixels of a window that vanished,
  shrank or moved survive. Use `loadOp=CLEAR` with `renderArea` set to the
  repaint rect (dynamic rendering clears only `renderArea`), or
  `vkCmdClearAttachments` with the repaint rects. This is a correctness
  requirement, not an optimisation, and its absence is a plausible contributor
  to `bf8e6950`'s failure — that commit changed only the repaint *selection* and
  left the recorder on LOAD.
- **Errors latch.** The canonical is never wholly recomposed, so there is no
  next-frame self-heal. Every one of these must force a full recompose plus
  all-BO full damage: `invalidate_bo`, failed submit, `DEVICE_LOST`,
  VT-switch/DPMS resume, modeset/RANDR resize, canonical (re)creation — and the
  easily-missed one: a **partial** frame. `record_and_submit_render` `break`s out
  of the descriptor-allocation loop on failure and records only
  `descriptors.len()` draws. Today that self-heals next frame; with damage
  retirement it would latch forever. Such a frame must not retire its damage.
- **Rect granularity.** `Repaint::Clipped` carries a single `vk::Rect2D`
  (bounding box). Decide explicitly whether to keep the bbox or carry a rect
  list. If bbox, telemetry must report bbox area, not summed rect area, or the
  win is overstated and "bounded scene damage" stops being a measurable
  criterion.

## Invariants that must not be lost again

These comments and two CI tripwire tests are on `master` and on this branch. The
abandoned branch **deleted all of them** and inverted the tripwires while
production still hardcoded `Repaint::Full` — so the guards went away and the
thing they guarded was never enabled. Amend them when the behaviour genuinely
changes; do not delete them.

1. **XOR root overlay is not idempotent.** `record_scanout_logic_fill` in
   `record_command_buffer` is correct today only under CLEAR + full redraw, so
   it XORs exactly once onto fresh pixels. As with the cursor invariant below,
   the rule that follows from that is *not* the same in the two designs, and the
   version on `master` is written for direct-to-BO repaint.

   - **Direct-to-BO buffer-age repaint**: the master invariant stands. A clipped
     LOAD frame whose scissor misses the overlay rects leaves a second copy
     XORed into a pooled BO that already has it baked in from a prior compose —
     the #90 rubber-band remnant. Fold `root_overlay.all_rects()` into the
     repaint region, or force Full when the overlay is non-empty.

   - **Phase 2 canonical composition**: the failure mode disappears, because a
     BO is only ever a copy destination and never an XOR target — nothing is
     ever baked into a pooled BO. The requirement becomes: **apply the current
     overlay clipped to the repaint scissor / render area**. Inside the repaint
     rect the pixels were just cleared and recomposed, so the XOR lands exactly
     once; outside it the canonical retains the previous frame's already-XORed
     pixels untouched. Overlay toggles and moves already damage old ∪ new rects,
     so a moving rubber band recomposes correctly through the ordinary damage
     path.

   Do **not** carry the fold-everything rule into Phase 2. Folding every active
   overlay rect into every frame would put the whole rubber band into the
   repaint region on every frame of a drag, defeating the optimisation on
   precisely the wireframe workload it is meant to serve. Note the coupling to
   §Rect granularity: a rubber band is thin lines with a huge bounding box, so
   under bbox-granularity repaint the fold costs the full screen, while under
   rect-list granularity it would merely be wasteful. That makes rect
   granularity worth more here than the raw pixel counts suggest.
2. **Stationary software cursor.** Cursor damage is gated out of
   `output_damage` at idle, which is safe today only because every compose is
   Full. What the fix is depends on which design is in play, and the two are
   *not* the same requirement — the invariant as written on `master` is
   specific to direct-to-BO repaint.

   - **Direct-to-BO buffer-age repaint** (anything in Phase 0 that clips
     straight into a scanout BO): the master invariant stands verbatim. The
     current SW cursor rect must be folded into the repaint region even when the
     cursor did not trigger the frame. The tripwire's own doc comment gives the
     reason — "*a fresh/older-age BO* shows a stale/missing cursor". That is a
     P1/recency argument.

   - **Phase 2 canonical composition**: per-BO fan-out already solves that
     recency argument. When the cursor was composited at position P, that
     canonical update unioned P into every BO's pending region, so every BO
     receives P before its next flip. A stationary cursor is therefore already
     correct in the canonical and in every BO, and an unrelated clipped repaint
     of region R need only handle the part of the cursor that R actually
     intersects. The weaker requirement that replaces it: **the SW cursor must
     be present in the scene draw list on every clipped compose**, so that where
     R clears and recomposes over the cursor rect, the cursor is redrawn. Fold
     the cursor rect into the repaint region only when the cursor *moved*.

   Do not delete `clipped_reenable_must_fold_in_stationary_sw_cursor_rect` when
   Phase 2 lands. Retarget it to the Phase 2 requirement and update its doc
   comment to record why the original reason no longer applies — the abandoned
   branch deleted this test and its sibling outright, which is how the reasoning
   above was nearly lost.

   Applies to the SW cursor path only; a HW cursor plane is not in the scene and
   is unaffected.
3. **Empty draw list.** Third instance of the same pattern; split it the same
   way.

   - **Direct-to-BO buffer-age repaint**: the master invariant stands. A scene
     that is only `bg_color` must stay Full, because a clipped LOAD frame
     preserves *each BO's* prior-generation content — including a pre-update
     background — outside the scissor, and nothing ever re-clears it.

   - **Phase 2 canonical composition**: an empty draw list does not by itself
     require Full. The repaint rect is cleared to the *current* `bg_color` and
     recomposed, so the new background appears wherever damage says it should.
     What the invariant becomes is a requirement on the producer instead: **a
     background change must itself produce full-output damage.** Forcing Full on
     an empty draw list would also destroy the clipped overlay-only case — a
     rubber band dragged over a bare desktop has no draws and a non-empty
     overlay, which is exactly the wireframe workload Phase 2 is for.

   Note the premise is currently vacuous: `bg` is a hardcoded
   `[0.0, 0.0, 0.0, 1.0]` at scene.rs:2837 and there is no root-background
   plumbing in the KMS scene path, so no `bg_pixel` change can reach it. The
   master comment anticipates this ("Stage 4 introduces root storage which makes
   the entry list non-empty even on blank desktops"). The forward requirement is
   therefore concrete: whoever plumbs root background must route the change
   through `mark_scene_structure_dirty()` (full-output damage) rather than
   `wake_for_damage()`, and Phase 0's ledger should carry a transition entry for
   it so a regression here is attributable rather than mysterious.
4. **Already-excluded hypotheses.** Input-coordinate hysteresis and
   invalidate-all-BOs-on-structure-change were both tried against drag-shake and
   both made it *worse*. Do not re-propose either without new measurement.

## Non-goals

- Direct scanout and Composite Overlay Window handling.
- The reverse-PRIME `Copied` output path. It keeps its current direct
  full-render path and is not canonicalised — see §Applies to
  `OutputScanout::Shared` only for the reasoning and for the cost of the
  recorder fork this implies.
- A public DAMAGE-extension dependency.
- Window bit-gravity / resize pixel preservation — separate issue, separate
  branch.
- Rect-precise scene-structure damage. Called out in §The corollary as the thing
  that would extend the win to window management, but it is its own project.

## Acceptance

- A Phase 0 report exists and qualifies P2 before any Phase 1 or Phase 2 code is
  written.
- **Static-desktop soak**: non-composited MATE, an untouched window, ≥10
  minutes, zero stale pixels. This is the exact regression that shelved
  attempt 2 and it is the primary gate.
- No stale pixels while moving/resizing MATE windows or opening shaped menus,
  with and without Marco compositing.
- Non-composited awesome/mpv: `full_redraw_fallback/s` well below
  `frame_present_count/s`, with the damage metric reporting the area actually
  repainted (see §Rect granularity).
- **No regression against `ad318afa` on the full-redraw path** — MATE idle and
  mpv frame times. Phase 1 adds a blit that never goes away on Full frames, and
  Full remains the recovery path.
- HW smoke before merge, run under lightdm→yserver rather than the `just *-hw*`
  harness.
