# Phase 0 implementation plan — damage-completeness audit

> **Context changed.** Phase 0 is no longer the project's gate — see
> `../specs/2026-09-01-damage-derived-scene-repaint-design.md`. The audit built
> from this plan is retained as the regression test for the derived-damage path.
> The plan below is still an accurate description of what was built.

Implements Phase 0 of
`docs/superpowers/specs/2026-09-01-noncomposited-damage-repaint-design.md`.
Read that spec first, in particular §P2 is the gate and §The candidate image
must be persistent.

**This is a diag branch. It is not a merge candidate and none of it ships.**
Branch `diag/damage-completeness-audit` off `fix/noncomposited-damage-repaint`.
Its only deliverable is a written finding under `docs/superpowers/findings/`
that either qualifies P2, produces an attributable list of missing producers, or
kills the project.

Because it is a diag branch, env-var gating is the right pattern here and does
not conflict with the no-kill-switches rule (that rule is about hedging
production features). Follow the existing shape: `tick_skip_log_enabled()`
(scene.rs:2020). Gate everything on `YSERVER_DAMAGE_AUDIT=1`; with it unset,
production behaviour must be observably identical to master — same damage, same
composes, same submits, same telemetry. (Not "byte-for-byte the same binary":
compiling the audit code necessarily changes that, and the claim worth holding
is about behaviour.)

## Two decisions to make before writing code

**The reference is a private image, not the scanout BO.** The obvious design
compares the candidate against the BO that production just composed. Don't.
Reading the BO means dealing with DMA-BUF imports, DRM modifiers, and the
scanout layout dance, and it couples the diagnostic to the very path whose
correctness is in question. Instead compose a *second private device-local
image* as the reference. Production's render into the BO is then untouched, the
audit cannot cause the artefacts it is measuring, and both images are ours so we
can give them whatever usage flags the compare shader needs. The cost is one
extra full compose per audited frame, which a diag branch can afford.

**Candidate every frame; reference and compare every frame too, for any run
that counts.** The candidate must be updated on *every* frame or it stops
modelling Phase 2. It is tempting to sample the expensive half — reference
compose plus comparison — via `YSERVER_DAMAGE_AUDIT_INTERVAL`, and the knob is
worth having because the audit may perturb timing enough to change behaviour.
But a divergence is **not** guaranteed to persist: later legitimate damage can
repaint the omitted area and heal it. A sampled run can therefore step straight
over a real failure and report clean.

So: `YSERVER_DAMAGE_AUDIT_INTERVAL > 1` runs **cannot qualify P2**. They are
supplementary timing experiments only, and the finding must label them as such.
Every run that feeds the exit decision runs at interval 1.

## Steps

### 1. Images and gating

Add to `OutputSceneState` (scene.rs:~462), behind the audit gate:

- `audit_candidate: CanonicalSceneTarget`-shaped image — device-local
  `B8G8R8A8_UNORM`, output extent, usage `COLOR_ATTACHMENT | TRANSFER_SRC |
  STORAGE`.
- `audit_reference: ` same.
- `audit_state: { initialized: bool, frame: u64, active_episodes: HashMap<TileId, EpisodeStart>,
  episodes_opened: u64, episodes_healed: u64 }` — per §5 this is *active* per-tile
  episode state, edge-triggered and cleared when a tile heals, plus counters for
  opened and healed episodes. Not a write-once first-divergence map.

The abandoned branch's `CanonicalSceneTarget::new` is a working reference for
the allocation (`git show a039b152 -- crates/yserver/src/kms/render/scene.rs`) —
read it for the memory-type selection, but write it fresh; do not resurrect the
branch.

Both images are created with the output and destroyed/recreated with it. Nothing
outside the audit path may reference them.

### 2. Where the audit hooks in — two paths, and the second one is the point

`tick_one_output` has an early return for empty damage at scene.rs:2279
(`TickOutcome::Skipped(TickSkipReason::EmptyDamage)`). Hooking the audit only
after the production compose would skip the single most important failure case,
so it must hook in **two** places.

**2a. The empty-damage path — hook this first.** A `wake_for_damage()` site that
should have contributed damage and did not produces exactly this outcome: the
scene is marked dirty so a tick runs, `output_damage` comes out empty, and the
function returns before composing anything. That is the archetype of the bug
being hunted, and the naive placement never sees it.

So: before the `EmptyDamage` return, whenever the ledger holds an unretired
transition for this output, run the audit. Leave the candidate **unchanged** —
zero damage correctly means zero candidate update, and mutating it here would
destroy the evidence. Compose the reference in full and compare. A mismatch here
is the cleanest possible result the audit can produce: a transition woke the
scene, reported no damage, and the screen should have changed. The culprit is in
the ledger with nothing else competing for the blame.

Do not let this path disturb the existing skip semantics — the tick must still
return `Skipped(EmptyDamage)`, still record the same telemetry, and still leave
`snapshots_carry_damage` and the forced-full-compose branch above it untouched.

**2b. The composing path.** After `output_damage` is finalised (scene.rs:2271)
and the production compose is recorded:

- If `!initialized`: compose the full scene into the candidate, set
  `initialized`, skip comparison this frame.
- Otherwise compose the scene into the candidate clipped to `output_damage`,
  using **`loadOp=CLEAR` with `renderArea` = the repaint rect** and the scissor
  set to the same rect. Not `loadOp=LOAD`.

The clear rule is load-bearing and is the single easiest thing to get wrong
here. With LOAD the candidate will also reproduce the missing-background defect
that Phase 2 §`loadOp=LOAD` alone is wrong describes, and the audit will report
it as a damage-completeness failure. Phase 0 must isolate P2.

Use the same rect-granularity decision that Phase 2 will use. If that is still
open, run the audit with the bounding-box form first, since that is the
conservative one — a bbox candidate repaints *more* than a rect-list candidate,
so any mismatch it finds is a real damage hole and not a granularity artefact.

### 3. Resets

Re-initialize the candidate with a full compose, and suppress mismatch counting
for that frame, on each of: modeset / RANDR resize, VT-switch and DPMS resume,
`DEVICE_LOST`, failed submit, `invalidate_bo`, and any partial frame — the
descriptor-allocation `break` in `record_and_submit_render` (scene.rs:~4239 on
the abandoned branch; find the equivalent on master).

These are exactly Phase 2's forced-full-recompose triggers. Wire them from one
shared helper so the two lists cannot drift; if Phase 0 does not reset on them it
reports their artefacts as damage bugs and the finding is worthless.

Log every reset with its cause. A run whose reset rate is high is not a clean
run, however few mismatches it reports.

### 4. GPU comparison

Model it on `crates/yserver/src/kms/vk/probe_digest.rs` — that is an existing,
working compute-reduce-then-read-a-small-summary pipeline, including the
`OUT_DIR` SPIR-V wiring, descriptor setup, host-visible summary buffer, and an
`is_supported()` gate. Reuse its structure; do not reuse the digest itself.

Write a new `damage_audit_compare.comp.glsl` in
`crates/yserver/src/kms/vk/shaders/` (build.rs compiles everything in that
directory automatically — no build wiring needed). It takes both images and
emits, per tile:

- a mismatch flag and a mismatching-pixel count;
- that tile's own first differing pixel index and both its candidate and
  reference colour values.

**Do not use a global `atomicMin` to publish the first differing pixel.** An
index and two colour words cannot be published atomically as a group: an
invocation can win the `atomicMin` on the index and then lose the race writing
the colours, yielding an index from one pixel and colours from another — a
plausible-looking report that sends the reader after a pixel that never
differed. Since probe_digest's structure gives each tile a single reducer
invocation, have each tile write *its own* first index and colours into its own
slot with no atomics at all, and select the global first on the CPU when reading
the summary. Same information, no race, less shader.

A direct compare beats two digests here: a digest says "this tile differs", a
compare hands you the coordinate and the two colours, which is what makes a
mismatch diagnosable rather than merely detectable.

Grid: reuse probe_digest's 64×64 cap. At 1080p that is ~30×16 px tiles. With
four `u32` per tile — mismatch count doubling as the flag, first differing pixel
index, and the two colour words — the summary is 4096 × 16 B = 64 KiB, matching
the budget probe_digest already documents. Cheap enough to read every frame.

Gate on an `is_supported()` check as probe_digest does, so CI's lavapipe path
and any device without the needed features degrade to "audit unavailable"
rather than failing.

### 5. Divergence tracking — edge-triggered, and healable

Per §Attribution must point at the frame divergence began, a report must cite
the frame a divergence *started*, not the current frame; correlating against the
current frame blames whatever happened most recently, which will usually be
innocent.

But do not model this as a permanent `HashMap<TileId, first_frame>`. A
divergence is not necessarily permanent: later legitimate damage can repaint the
omitted area and heal the tile. A write-once map would then suppress a
subsequent, genuinely independent failure in the same tile — and tiles covering
busy screen regions are exactly where repeat failures are likely.

Track each tile's **current** matched/mismatched state and report every
`matched → mismatched` **transition**, clearing the active state on
`mismatched → matched`. Edge-triggered, so the log stays readable, and each
episode is reported with its own start frame.

Count healed episodes separately and report them. A short-lived divergence is
still a damage hole; it merely got papered over by unrelated later damage, which
is luck, not correctness. Under Phase 2 the same hole against a different
workload may not get so lucky.

### 6. Transition ledger

Two pieces, and the first one is nearly free.

**Call-site identity via `#[track_caller]`.** Put `#[track_caller]` on
`wake_for_damage()` (scene.rs:827), `mark_scene_structure_dirty()` (scene.rs:835),
and the `mark_scene_structure_damage_rect{,s}` pair, and capture
`std::panic::Location::caller()`. That gives file:line for all 22
`wake_for_damage()` sites and all ~20 `mark_scene_structure_dirty()` sites
immediately, with no hand-labelling and no risk of mislabelling.

**The ledger proper.** A `Vec<LedgerEntry>` on `SceneCompositor` with
**monotonic** event IDs and a **per-output consumed cursor** — *not* a
frame-local list cleared at tick start.

```
LedgerEntry { id: EventId, site: &'static Location<'static>, drawable: Option<DrawableId>,
              old: StateSnapshot, new: StateSnapshot, expected_area: Vec<Rect> }
```

Clearing at tick start loses and misassigns events. Transitions accumulate
before `tick()` is entered; outputs are then processed sequentially in the
`for output_idx in 0..n_outputs` loop (scene.rs:1226); and any individual output
can skip for a pending flip, a retry deadline, an unavailable BO, or empty
damage. An event cleared on behalf of an output that never audited is an event
whose failure is now invisible, and one attributed to the wrong output's frame
is worse than none.

So: retain entries, and give each output a cursor over the monotonic ID space.
An output retires events up to its cursor only after it has actually been
audited. The codebase already uses this exact shape one level up —
`clear_dirty &= outcome.clears_scene_structure_dirty()` (scene.rs:1244) only
clears the dirty flag if *every* output agreed. Follow it; a monotonic ID plus a
cursor is simpler than frame-local IDs and cannot misassign.

Bound the retained set so a permanently skipping output cannot grow it without
limit — drop the oldest with a logged warning, and treat any run that hits the
bound as suspect rather than clean.

`StateSnapshot` grows as needed — geometry, shape serial, stacking position,
source version, cursor position and version, map/redirect state.

Stage this. Ship step 6 with call site + `EventId` + a coarse `expected_area`
only; that is enough to answer "which site failed to report". Add richer
old/new state to specific sites once the audit names a suspect. Front-loading
full state capture at 40+ call sites before knowing which matter is wasted work.

On `expected_area`: it may be deliberately dumb, over-broad and slow — whole old
bbox ∪ whole new bbox, no clipping, no occlusion reasoning. It never ships. The
objection "if you can compute it, use it as the damage" is answered in the spec:
production damage has holes precisely *because* it must be tight and cheap.

**Contributions live in a side table that is never authoritative.** Record
`contributions: Vec<(EventId, Vec<Rect>)>` per output per frame, **purely for
reporting**.

The existing `RegionSet` operations stay exactly as they are, audit enabled or
not. Do **not** derive `output_damage` by unioning the side table, and do not
refactor damage computation to flow through it: the diagnostic would then be
altering the very value it exists to audit, and a bug in the side table would
present as a damage bug. The audited quantity must be computed by untouched
production code.

Nor should provenance go inside `RegionSet`. `add()` collapses to a bounding
rect above `MAX_RECTS` (store.rs), which would silently desynchronise any
parallel provenance vector.

The invariant to hold: with `YSERVER_DAMAGE_AUDIT` unset the binary is
byte-for-byte current behaviour, and with it set, `output_damage` is bit-identical
to what the same run would have produced with it unset.

### 7. Attribution join and report

For each newly-mismatching tile, at its first-divergence frame:

- ledger entries whose `expected_area` covers the tile;
- for each, whether that `EventId` appears in `contributions` at all;
- the tile's first differing pixel plus both colours;
- the scene draws overlapping the tile, with xids.

The finding is the set of `EventId`s whose expected area covers a mismatch and
which contributed nothing. Expect `wake_for_damage()` sites to dominate — by
construction they never contribute, and their whole safety argument is that
per-drawable presentation damage covers them. That argument is the thing under
test.

Emit machine-readable lines (TSV, one per first-divergence) so runs can be
diffed across sessions, following
`docs/superpowers/findings/2026-06-18-xorg-xts-baseline.tsv`.

### 8. Detail capture

On first divergence, optionally dump both full images plus the damage rects and
the frame's ledger to disk, rate-limited to a handful per run
(`YSERVER_DAMAGE_AUDIT_DUMP=<dir>`). Cheap to add, and it is the difference
between "tile 41,7 differs" and seeing what it looks like.

### 9. Run matrix

Per the spec, and per
`feedback_verify_via_display_manager_before_investigating`: run under
**lightdm→yserver**, not the bare `just *-hw*` harness, for anything involving
real desktop apps. `just yserver-mate-hw` is fine for the MATE compose cases.

- non-composited MATE: menus, window drag, interactive resize
- MATE with Marco compositing on, as a contrast case
- awesome
- i3 with a floating-window drag (rubber band / wireframe overlay active)
- mpv
- **static desktop soak** — untouched window, ≥10 min. This is the case that
  shelved attempt 2 and it is the primary gate. Run it longest.

Vary one thing at a time. Record reset counts and audit interval alongside every
result; a run without them cannot be compared to another run.

### 10. Report and decide

Write `docs/superpowers/findings/<date>-damage-completeness-audit.md` with the
TSV alongside it, then take the spec's exit branch:

- clean across the matrix ⇒ P2 **qualified** (not proved — say so), Phase 1 is
  justified;
- bounded attributable list ⇒ fix those producers, re-run, then Phase 1;
- unbounded or unattributable ⇒ **the project stops**, and the finding says so.

The third outcome is a real result, not a failure. Recording it is worth more
than a fourth attempt at the same guess.

## Do not

- Do not enable clipped repaint in production anywhere in Phase 0.
- Do not add a guard band, or inflate damage, to make mismatches go away. That
  is the signal a producer is missing. Three attempts have now ended here.
- Do not delete or invert the `master` tripwire tests
  (`pick_repaint_returns_full_while_optimisation_disabled`,
  `clipped_reenable_must_fold_in_stationary_sw_cursor_rect`) or the hazard
  comments they guard. The abandoned branch did exactly that.
- Do not touch the `OutputScanout::Copied` path.
- Do not let the audit change production damage computation or the production
  render path when `YSERVER_DAMAGE_AUDIT` is unset.

## Verification for the diag branch itself

`cargo +nightly fmt`, `cargo clippy --all-targets -- -D warnings`,
`cargo test -p yserver`. Plus one specific check, because the audit is only
trustworthy if it can see a bug: **inject a known damage hole** and confirm the
audit reports it, with the right start frame and the right `EventId`. An audit
that has never been seen to fail has not been tested.

Inject it correctly: suppress only the **damage contribution**, while leaving
the wake/dirty scheduling intact. Deleting a whole `mark_scene_structure_dirty()`
call also removes the reason a tick runs at all, so the audit may simply never
fire — and a test that passes because nothing happened proves the opposite of
what it claims. Keep the site marking the scene dirty; drop only the rect it
adds. That also exercises path 2a, which is where the real
`wake_for_damage()`-shaped bugs will surface.
