# Stop repainting the whole screen — design

**Supersedes** `2026-09-01-noncomposited-damage-repaint-design.md`. That spec is
kept for the record: its §Prior attempts, §Invariants register and the Phase 0
audit it produced all remain valid and are referenced below. What it got wrong
was the reference implementation.

Reviewed by codex 2026-09-01, two rounds; nine changes applied — all-affected-node
visibility re-derivation, transactional ring rotation, the cursor and
root-overlay carve-outs, opaque ∩ visible in the background algebra, an
explicit multi-rect rendering decision, and promoting COW/redirect from an open
question to a prerequisite. Round 2: the per-BO damage model respecified
(the first version described the wlroots ring wrongly), subtree rather than node
bounds, and content-damage visibility clipping as the intended final path.
Round 3: the visibility/opacity table filled from current behaviour, the
opaque-region rule, and a constraint against retiring `suppress_cow` before its
replacement is tested.

## Why this work exists

yserver repaints the entire output every frame. Xorg, non-composited, repaints
only what changed — and on the hardware where this hurts (z400 + RX 460,
2560×1440) that is the difference between **3-6.3 ms of GPU per compose** and
almost nothing, measured. At ~44 fps that is 18-28% GPU spent redrawing pixels
that did not change; an RX 570 at 4K measures 6.8 ms.

The control is the important part: **Xorg does the same workload on the same
card, into the same LINEAR scanout buffer, for almost no cost.** So the expense
is not intrinsic to the hardware, the driver, or the buffer layout — it is the
pixel count. LINEAR raises the price per written pixel and so amplifies the
gap, but it is neither the cause nor the fix; do not go chasing modifiers.

Everything below exists to write fewer pixels per frame.

## Reference implementation

### Why it is wlroots and not Xorg

The superseded spec was built on Xorg modesetting's TearFree path. Xorg is the
right reference for protocol semantics and it is not the right reference for
render architecture, because the two servers are built differently in the one
way that matters here:

- **Xorg**: clients draw *into* the screen pixmap through the clip/validate
  machinery. The authoritative desktop image therefore exists for free, is
  correct by construction, and its DAMAGE region is recorded by the same code
  that does the drawing. TearFree just copies rectangles out of it.
- **yserver**: every window owns its storage and the desktop is *recomposited*
  from a scene graph each frame. There is no such image, so the old design
  proposed manufacturing one — at the cost of a full-screen blit every frame,
  forever, on a path where `Repaint::Full` remains the fallback.

wlroots is in yserver's architecture exactly: per-surface buffers, a scene
graph, composited per output per frame. It has the same problem and solves it
without an authoritative image at all.

### What wlroots actually does

Verified against `../wlroots` 0.20.2.

1. **Damage is derived, never declared.** Mutators do not know about damage.
   `wlr_scene_node_set_position` is the whole pattern: assign `node->x/y`, then
   `scene_node_update(node, NULL)`. `scene_node_update` (wlr_scene.c:722) takes
   the node's *previous* visible region as the damage, unions its *new* bounds,
   re-derives visibility for everything intersecting that region, and damages
   the outputs — old ∪ new, automatically. Every mutator does this and nothing
   else.

2. **Each node caches an occlusion-aware visible region.**
   `scene_node_update_iterator` (wlr_scene.c:582) walks nodes front-to-back over
   the update box carrying a running "still visible" region: each node's
   `visible` becomes that region clipped to its bounds, then the node's *opaque*
   region is subtracted from the running region so nodes below see less.

3. **Per-buffer damage accumulation, keyed by buffer identity — not age.**
   `wlr_damage_ring_rotate_buffer` (wlr_damage_ring.c:78): the damage for the
   buffer being rendered is `current` ∪ the stored damage of every *other*
   tracked buffer; that buffer's stored damage then becomes `current`, and
   `current` clears. An untracked buffer gets whole-buffer damage.

4. **No canonical image.** `wlr_scene_output_commit` acquires a buffer from the
   swapchain and calls `wlr_renderer_begin_buffer_pass` directly on it
   (wlr_scene.c:2490, 2520). The scene graph is the source of truth; each frame
   re-rasterises the damaged region straight into the scanout buffer.

5. **Background is rendered inside the damage region, minus opaque coverage.**
   `background = damage`, then the opaque region of each node above is
   subtracted before the background is drawn (wlr_scene.c:2536-2560).

6. **Rect list with a cap.** Damage is kept as rectangles up to
   `WLR_DAMAGE_RING_MAX_RECTS`, then collapsed to extents.

## Design

**The one thing that saves the GPU is repainting only the damaged region
instead of the whole output.** Everything else here exists to make that safe and
worth doing:

| step | what it does | why it is needed |
|---|---|---|
| **1. Know what each window covers** | an occlusion-aware visible region per scene node | without it we cannot tell which pixels a change actually affects |
| **2. Damage falls out of scene changes** | derived on mutation instead of declared by hand | today's damage is either missing or the whole screen, so there is nothing tight to clip to |
| **3. Track what each scanout buffer is missing** | per-BO accumulated damage | a recycled buffer is older than the last frame; without this a partial repaint leaves stale pixels |
| **4. Repaint only the damaged region** | clipped compose straight into the BO | **this is the step that cuts the GPU load** |

Step 4 is the deliverable. Steps 1-3 are what make it correct and give it
something tight to clip to. Each is independently testable, and steps 2 and 3
can land while rendering stays full-screen, so they cannot cause a visual
regression on their own.

### Step 1. Know what each window covers

Give every scene participant a cached visible region, computed front-to-back
with opaque subtraction as in (2). "Opaque" for us means depth-24 (or
alpha-free) storage with no bounding/clip shape and no translucent stacking.

This is the missing primitive, and it is load-bearing for everything after it.
It is also the fix for the standing bug where the scene walk clips children by
the parent's rect only, so a parent's empty bounding shape punches no hole and
`suppress_cow` is doing that job by hand.

### Step 2. Damage falls out of scene changes

**Superseded by the implementation plan, 2026-09-01 (codex round 5).** The
mechanism below is wlroots': mutators call one update function that reads the
node's *cached previous* visible region. That rests on a persistent scene graph,
and yserver has none — `build_scene` (scene.rs:4058) is a stateless walk called
fresh per output per frame, reconstructing the scene from the windows map and
dropping it. There is no node to cache a region on, and building a persistent
mirror would create a second source of truth invalidated by a dozen inputs, five
of which are mutated today with no damage call at all. The plan replaces this
with a **tick-time diff of the emitted draw list**, which reproduces the same
invariant by construction. The reasoning below about *what* damage is owed —
old ∪ new, subtree not node, and the two producers that survive — remains
correct and is what the diff is checked against.


One `scene_node_update`-equivalent. Every geometry, map, unmap, restack,
reparent, shape and redirect change routes through it.

**Two distinct regions, and conflating them is the easy mistake.** wlroots
(wlr_scene.c:745-757):

```
update_region = old_subtree_visible(n) ∪ new_subtree_bounds(n)
    scene_update_region(update_region)      # mutates cached visible for
                                            # EVERY node in the region
damage        = old_subtree_visible(n) ∪ new_subtree_visible(n)
```

**Subtree, not node.** Both `scene_node_bounds` and `scene_node_visibility`
recurse into tree children (wlr_scene.c:612, 625). Using the node's own bounds
would miss children that extend beyond the parent — which for us is not a corner
case: it is the COW subtree, and it is the standing parent-shape bug that step 1 is
meant to fix.

`update_region` uses *bounds*; the posted `damage` uses the *visible* region,
after the re-derive. The re-derive is not optional bookkeeping
— `scene_node_update_iterator` rewrites `node->visible` for every node in the
region, which is how a sibling revealed from under a moving window gets a
correct visible region for the *next* mutation.

Posting only the mutated node's old ∪ new visible is sufficient for *this*
frame, because only that node's coverage changed, so every other node's
visibility delta lies inside it. That is a real invariant and it is worth
stating, because it is not obvious and an implementation that "plays it safe"
by damaging every changed node's old ∪ new is doing redundant work.

Any implementation that updates the mutated node's visible without re-deriving
its neighbours' is wrong, and will fail on the second mutation, not the first.

### What step 2 does and does not replace

It replaces ordinary structural damage: the ~20 `mark_scene_structure_dirty()`
whole-output sites and most of the ~22 region-less `wake_for_damage()` sites.
That is what makes drag and resize cheap.

It does **not** replace these two, and a plan that assumes it does will delete
working correctness:

- **Cursor.** Cursor damage is tick-owned and transactional today — computed in
  `tick_one_output` from `last_present_cursor_rect`/`version` against the
  frame's new rect (scene.rs:3357) and retired through `PendingAck`. Either the
  SW cursor becomes a real scene node with ordinary visible/damage semantics, or
  it stays a separate producer. Decide explicitly; do not let it fall between.
- **Root `IncludeInferiors` XOR overlay.** Non-idempotent, and it injects
  explicit rect damage today (`root_overlay_toggle`, scene.rs:1241) precisely
  because `wake_for_damage` alone leaves the frame EmptyDamage-skipped. See the
  invariants register.

Content damage continues to come from per-drawable presentation damage, which
Phase 0 measured as complete for a paint workload (8999 partial comparisons,
zero mismatches — see the finding).

It may over-damage initially: `add_projected_damage` (scene.rs:4912) projects
storage-local damage into output coordinates with no visibility clipping, so a
paint in an occluded part of a window damages screen area that cannot have
changed. That is safe, and fine for a first cut. Once step 1 exists the intended
final path clips projected content damage to the node's cached visible region —
worth doing, since a mostly-covered window repainting continuously is exactly
the workload where the win would otherwise leak away.

### Step 3. Track what each scanout buffer is missing

Per-scanout-BO accumulated damage keyed by BO identity. Untracked or
invalidated BO ⇒ full damage. Rect list with a cap, then extents.

**The rotation must be transactional — this is where a direct port breaks.**
wlroots rotates destructively at acquire because its commit flow is tight around
the acquired buffer. yserver's is not: BO state is committed only at page-flip
retirement (`commit_bo_present`, scene.rs:1732), and both submit-failure paths
explicitly "fold repaint forward and do NOT push a pending_ack or advance
current_generation" (scene.rs:3757).

So a ring that clears `current` at acquire will lie whenever the KMS commit
later fails — the damage is gone from the ring and was never presented. That is
the same shape as the buffer-age corruption that got earlier attempts reverted.

### Do not port wlroots' ring structure — port the guarantee

wlroots keeps an MRU list where `entry->damage` is *interval* damage: the
damage that accrued after that buffer was rendered and before the next one was.
`wlr_damage_ring_rotate_buffer` computes

```
damage = current
for entry in buffers:  if entry.buffer != buffer:  damage |= entry.damage
```

— that is `current` **plus every OTHER buffer's interval**, and specifically
*not* the selected buffer's own entry. Rotation then calls
`entry_squash_damage` to merge the outgoing interval into the *previous list
entry* before overwriting it, and moves the entry to the head
(wlr_damage_ring.c:58, 78).

That squash-into-previous step couples correctness to list recency, which is
precisely what cannot be staged and rolled back cleanly. Since yserver *must*
stage (BO state commits at retire, and failure paths deliberately do not
advance), port the guarantee rather than the data structure.

### The model to implement

**Correction, 2026-09-02: the region type this section names does not exist.**
`RegionSet` (store.rs:499) is a `Vec<Rect2D>` whose `subtract` removes **exact
rect matches** as a multiset. That is right for the snapshot/ack path — the
`355c221f` case cited below — and it cannot express `missing[X] -=
submitted_repaint`, which is geometric subtraction of one region from another.
Used as written it would match nothing, `missing[X]` would grow to its 256-rect
cap, collapse to extents, and pin every frame to `Repaint::Full` for ever:
safe, silent, and useless. A banded region type with real union / subtract /
intersect is therefore a **prerequisite for this section**, added to the
implementation plan as *step 0* (`../plans/2026-09-01-damage-derived-scene-repaint-plan.md`).
`RegionSet` stays as it is for the store's presentation damage; the plan
enumerates the conversion boundary.

With a real region, `pending.subtract(submitted_pending)` below is also wrong —
geometric subtraction would delete post-submit damage wherever it overlaps what
was submitted, which is the very thing that rule exists to protect. The plan
replaces it with stage-and-restore into a single in-flight slot, and replaces
`submitted_repaint` with the region actually *painted*.

Per output, two pieces of state with one invariant:

- `pending: RegionSet` — damage accrued and not yet attributed to any BO.
- `missing[bo]: RegionSet` — **invariant: the pixels of `bo` that do not
  reflect the current scene.**

Everything follows from that invariant:

- **New damage `D`** → `pending |= D`.
- **At acquire of BO X** → `repaint = pending ∪ missing[X]`. Compute only, no
  mutation. Untracked or invalidated BO ⇒ `missing[X] = full output`.
- **Stage** `submitted_pending` and `submitted_repaint` on the frame's
  `PendingAck`, alongside the existing `submitted_output_damage` /
  `submitted_scene_structure_damage` snapshots.
- **At successful retire** →
  - for every tracked BO `Y != X`: `missing[Y] |= submitted_pending`
    (the new damage is now stale in Y too);
  - `missing[X].subtract(submitted_repaint)` (X now reflects what we painted);
  - `pending.subtract(submitted_pending)` — **subtract, never clear**, so
    damage that arrived between submit and retire survives. This is the
    `355c221f` lesson, and `RegionSet::subtract` is already multiset for it.
- **On failure** → no mutation at all. `pending` keeps everything, `missing[X]`
  is untouched, and the next acquire recomputes correctly.

Cap the rect count as wlroots does: above the cap, collapse a region to its
extents.

This is simpler than wlroots' structure and strictly easier to verify — every
operation is checkable against the one invariant, and the failure path is
"do nothing" rather than an unwind.

### Step 4. Repaint only the damaged region

Render the scene into the acquired BO clipped to that BO's accumulated damage.
This is the concrete answer to the old spec's "`loadOp=LOAD` alone is wrong"
problem.

**Background algebra — intersect opaque with visible before subtracting:**

```
background = damage − ⋃ ( opaque(n) ∩ visible(n) )
```

wlroots does exactly this (wlr_scene.c:2549): each node's opaque region is
intersected with that node's *cached visible region* before being subtracted.
Subtracting raw opaque bounds instead would suppress background under areas the
node does not actually cover — a covered or optimised-away opaque region would
punch a hole the background never fills, which reads as a black or stale patch.

**Rect granularity — decide now, do not leave it implicit.** The damage ring
carries a capped rect list, but today's recorder is single-rect: one
`Repaint::Clipped(vk::Rect2D)` and one scissor. Two options:

1. **Capped region for accounting, bbox for rendering** (recommended first
   cut). The ring keeps rects so the *accounting* and telemetry are honest;
   rendering uses the bbox. Correct, strictly simpler, and it lets A+B be
   validated before the recorder changes. The cost is over-repaint on scattered
   damage — which the telemetry will show, since `mean_damage` is computed from
   the real rects rather than the bbox.
2. **Real region clipping** — per-rect scissor passes, or a scissor array.
   Better, but it touches the recorder and every op that assumes one clip rect.

Take (1) first and (2) only if the measured over-repaint justifies it.

**Clipping needs a damage-fraction threshold: above roughly 60-70%, render
Full.** Measured on bee, a clipped compose at `mean_damage=0.857` cost *more*
than a full one (208.7 µs vs 199.3 µs) — a sub-rect `CLEAR` plus scissor setup
does not pay for itself at high damage fractions. Without the threshold, clipped
repaint is a net loss on precisely the frames that are whole-output today. See
the finding for the numbers.

**Size of the prize.** On a fast GPU at 1440p with tiled buffers a full compose
is ~440-520 µs and clipping roughly halves it — ~1.5% of a 60 Hz frame, weak.
**That is not the hardware this work exists for.** On a modifier-less Polaris
card (LINEAR scanout) at 4K, a compose is **6.8 ms — ~40% of the frame budget**
— with mpv windowed at ~6% of the screen area (discussion #56). Tight damage
there implies roughly a 1 ms compose: 40% of budget down to ~6%.

This is why steps 3 and 4 are the point of the work rather than a follow-on: yserver
repaints the whole screen every frame, and non-composited Xorg does not
recomposite at all. The gap scales with pixels, with how slow the GPU is, and
with LINEAR buffer layout — i.e. it is worst exactly on the hardware most likely
to be running yserver.

Note also that `damage_fraction` in the existing telemetry is 1.000 by
construction on the always-Full path and measures nothing; only the audit's
`mean_damage` measures real damage extent.

## What this removes from the superseded design

- **The canonical scene image, and with it all of old-Phase 1.** It was a
  full-screen device-local image per output plus a full-screen blit every
  frame, producing identical pixels. Pure cost.
- **The bounding-box-versus-rects open question.** Answered: rects with a cap
  for accounting; see step 4 for the rendering-side decision, which is separate.
- **Phase 0 as the project's gate.** See below.
- **`mark_scene_structure_dirty`'s whole-output hammer**, and therefore the
  "clipped repaint gains nothing during window management" corollary. Under step 2
  a drag damages old ∪ new, not the screen.

## What carries over unchanged

The superseded spec's §Prior attempts and §Invariants register. One correction
to the register: three invariants (stationary SW cursor, XOR root overlay,
empty draw list) were weakened there on the grounds that *a canonical image*
subsumes the buffer-recency argument they rest on. There is no canonical image
now — but the weakenings still hold, because the argument that actually
subsumes them is **per-BO damage accumulation** step 3, which survives. Same
conclusion, corrected reason. Do not re-weaken anything on the strength of a
component this design no longer contains.

## What Phase 0 becomes

Not the gate. Damage completeness stops being an empirical question about
forty-two call sites and becomes a structural property of steps 1 and 2.

The audit is not wasted: it becomes the **regression test** for the derived
damage path, which is exactly what is wanted when replacing forty-two call
sites with one mechanism. It is already built, already validated against an
injected hole, and already carries the `idle`/`partial`/`full` classification
needed to tell a real clean run from a vacuous one. Its one unexplained result
(the frame-2 divergence) should still be resolved, because it is a live
question about whether a compose can sample storage whose paint has not landed
— which matters under any design that does not fully recompose every frame.

## Prerequisite: the visibility/opacity table

**Written; this was the last gate before an implementation plan.** step 1 is
the foundation for everything else, and it is wrong for the whole design if
redirected and COW nodes are not modelled from the start. The current scene walk
already carries non-trivial COW suppression, redirected-backing routing and
alpha passthrough (scene.rs:4111, scene.rs:4501) — that behaviour encodes
decisions that step 1 has to reproduce, not discover.

Define, for each case, both *what region the node occupies in the scene* and
*whether it is opaque for occlusion purposes*:

Filled from **current behaviour**, verified against the scene walk — this
records what yserver does today, which is what step 1 must reproduce, not an
idealisation of it.

**Correction, 2026-09-01 (codex round 3).** The manual-redirect row originally
said the walk prunes the whole subtree, sourced from the module doc at
scene.rs:26. That module doc is stale and the code says so: *"the old
`prune_subtree=true` for `scene_participating=false` is gone — Automatic
descendants of Manual ancestors need to recurse so they can emit their own
backing"* (scene.rs:4543-4548). The prune was removed by audit #3 because it
dropped GTK/marco CSD frames' inner widgets. The row below is corrected; the
implementation plan replaces this table's per-case reasoning with the general
rule **occlusion follows emission**, which gets every row right including this
one. A comment is not the code — see the plan's step 1.

| case | contributes to scene | opaque **region** |
|---|---|---|
| normal unredirected window | yes — mapped, `scene_participating`, `DrawableKind::Window`, intersects the output; own storage | per the rule below |
| automatic-redirected window | yes — source routed through `store.redirected_target(id)` (scene.rs:4013, 4501), but host window geometry and shape | per the rule below, applied to the host window, and only if the backing samples opaque |
| manual-redirected window | **the node, no; its subtree, yes** — the node is skipped (`is_manual_redirected`, scene.rs:4628) but the walk recurses, and a descendant owning its own `redirected_target` emits its backing (`paint_target_is_self`, scene.rs:4629) | **empty for the node** — it must never occlude what the compositor will draw from its backing. Its emitting descendants occlude normally |
| COW subtree | yes — real root child, emitted with `alpha_passthrough = true` (scene.rs:4735, 4760) | **empty** — blended content |
| unredirected fullscreen window above the COW | yes — own storage | per the rule below; when it covers the output this is the principled replacement for `suppress_cow` |

### Three rules that fall out of it

**Be conservative about opacity.** A false negative costs overdraw. A false
positive punches a hole the background never fills, which shows as a black or
stale patch. When unsure, not opaque.

**"Contributes to scene" and "occludes lower nodes" are independent
decisions.** Manual-redirected windows are the clearest case: they have storage
and a backing, and they must still not hide anything, because the compositor
is going to paint those pixels itself. Model them as two separate predicates
from the start; a single `visible` flag will collapse them and the bug will
surface as a compositor's window vanishing under its own client.

**Opacity is a region, not a boolean.** Today's only opacity test is
`suppress_cow`'s `g.depth != 32` (scene.rs:4118) — a boolean, which is adequate
there because that path already requires a window covering the whole output,
where shape is a non-issue. General occlusion is different: a shaped window is
opaque *within its shape*, and treating shape as disqualifying would discard
occlusion for every rounded-corner menu and panel.

The rule:

| node | opaque region |
|---|---|
| unshaped, no alpha channel (depth ≠ 32) | the whole visible window region |
| shaped, no alpha channel | `visible ∩ shape` |
| depth-32 / alpha-capable / `alpha_passthrough` / COW-blended | **empty**, unless proven otherwise |

Then follow wlroots exactly: intersect the opaque region with the node's cached
`visible` before subtracting it — from the running visibility region in step 1, and
from the background in step 4. `scene_node_opaque_region` returns a region and
wlr_scene.c:2549 does that intersection; skipping it lets a covered or
optimised-away opaque area suppress coverage it does not actually own.

`suppress_cow` is the tell that this design is the right shape: it already
computes covers ∧ opaque ∧ participating by hand for one special case
(scene.rs:4111-4125). Under step 1 that is not a special case, it is what
occlusion culling does for every node.

## Open questions

- **Opaque-region derivation for ordinary windows.** Depth and shape we have;
  translucent stacking and ARGB visuals need care. Over-culling shows as
  missing background.
- **The `Copied` reverse-PRIME path.** Out of scope as before; state explicitly
  whether the ring applies there.
- **Cost of step 1.** Visibility is re-derived per mutation over the update
  region, not per frame over the screen. That should be cheap, but a drag
  mutates every frame and the region walk is not free — worth measuring, not
  assuming.

## Acceptance

### The success measure is GPU load against a wlroots compositor

**Baseline measured: labwc does this workload for 2.8% GPU on the z400's
RX 460, with mpv itself at 3.9%.** yserver's compose alone is 13-28% on the same
box. That 2.8% is the number to reach; see the finding for the data.

Everything else in this section is a correctness gate. **The measure of whether
this project succeeded is GPU utilisation on the z400 + RX 460 running windowed
mpv, compared against a wlroots compositor (sway or labwc) on the same box and
the same clip.**

Why wlroots and not Xorg. Xorg non-composited does *no compositing pass at all* —
clients draw into the screen pixmap and a Present copies into the framebuffer —
so its near-zero cost is not a target a scene-graph compositor can reach, and
falling short of it would not distinguish a bug from the architecture. wlroots
has our model: per-surface buffers, a scene graph, a compositing pass per frame.
Whatever it achieves on that card is what we should achieve.

It is also the better control. A wlroots compositor on the same modifier-less
Polaris hits the **same LINEAR constraint** we do, so hardware and buffer layout
are held constant and any remaining gap is our pixel count. Keep Xorg as a
secondary reference — it is the compatibility target and the comparison users
will actually make — but do not treat it as the engineering goal.

Run it windowed, not fullscreen, on both sides: fullscreen invites direct
scanout and stops measuring the compositing path. Note that Wayland surface
commits and X11 DRI3/Present differ on the client side; the comparison is
compositor-side GPU work, so read total GPU utilisation rather than trying to
match per-frame accounting.

Proxies that must not be mistaken for the measure: `mean_damage`, µs per
compose, partial-comparison counts, frame time. Each of them misled this project
at least once — µs-per-compose most badly, because the audit composes tiled
while production composes LINEAR.

### Correctness gates

- No stale pixels: non-composited MATE menus, drag, resize; awesome; i3
  floating drag; shaped windows.
- The audit reports zero mismatches with a **non-trivial `partial` count** —
  a clean run whose comparisons were all `full` or `idle` is not evidence.
- Drag and resize produce *partial* damage, not whole-output. This is the
  behavioural signal that step 2 actually replaced the hammer, and it is directly
  observable as `mean_damage` well below 1.0 during window management.
- No regression against `ad318afa` on the full-redraw fallback path. Note this
  is a correctness/perf guard, not the success measure — see above.
- The standing parent-bounding-shape bug is fixed by step 1, or explicitly is not.
- HW smoke under lightdm→yserver before merge.

## Order of work

**Superseded by the implementation plan, 2026-09-02.** The order is now
**0 → 3 → 4 → 2 → 1**, where step 0 is the banded region type this spec assumed
it already had (see the correction in §The model to implement). Step 1 stopped
being a prerequisite for step 2 once step 2 became a tick-time diff, and step 4
leads because it is the only step that cuts GPU load — everything before it is
bookkeeping the always-Full path ignores. The reasoning is in
`../plans/2026-09-01-damage-derived-scene-repaint-plan.md` §The order decision.

What still holds from the original: **the goal is step 4: stop repainting the
full screen.** Judge every decision here against that, and against the
acceptance measure (GPU load versus labwc on the z400), not against the internal
tidiness of steps 1-3.

The visibility/opacity table above precedes all of it — with the manual-redirect
row corrected.

**Do not delete `suppress_cow` in the same change that introduces step 1.** It is
the only thing keeping fullscreen-over-COW correct today, and it is a
one-output, fully-covering special case of the question step 1 answers generally.
Land step 1 with `suppress_cow` still in place, add explicit tests pinning the
current COW behaviour, and retire the hand-rolled path only once those tests
show the general mechanism produces the same result. The failure mode if this
is rushed — a compositor overlay wrongly suppressed or wrongly retained — is a
black screen, and it is the exact area where `project_scanout_m2_*` already
records a fatal guard bug.

**Open: whether steps 3 and 4 can go first.** The order below was written
before the labwc baseline existed. The mpv case may not need step 2 at all —
a windowed client's Present damage should already be tight in `output_damage`
today, in which case steps 3 and 4 alone deliver the GPU win and step 2 becomes
the follow-on that extends it to drags and resizes. One measurement decides it:
`mean_damage` for windowed mpv on the z400. Near 0.06 ⇒ start with steps 3 and
4. Near 1.0 ⇒ something forces whole-output damage even for a windowed video,
and step 2 must lead.

Steps 3 and 4 are separable from each other: Step 3 is bookkeeping with a
transactional contract and can be landed and tested against always-Full
rendering, which is a good way to prove the staging/retire logic before any
pixels depend on it.
