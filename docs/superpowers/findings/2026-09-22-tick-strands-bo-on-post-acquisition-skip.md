# The compose tick strands a scanout BO on any post-acquisition skip

**Status:** confirmed on hardware 2026-09-22, during plan Cp task 8's
cross-device run. Upstream code; **we own the fix too** (user, 2026-09-22:
"si nos afecta, también es responsabilidad nuestra").

**Where:** `crates/yserver/src/kms/render/scene.rs`, the composite tick.
Blame: Jos Dehaes, commit `02bafec3` (2026-09-03, "damage-clipped repaint for
non-composited desktops"). The tick is shared by every route — Legacy and
Owner, shared and copied pools — so this is not C.0-specific and it affects
production today.

## The defect

`acquire_managed_scanout_bo` / `acquire_scanout_bo` own the destination's phase
transition `Free -> Recording` (`platform.rs:6374`, with B-13's comment saying
exactly why the tick must own it). Several fallible steps run **after** that
acquisition, and at least one of them returns without restoring the phase:

- the descriptor-pool ring exhaustion branch returns `Skipped(NoPool)` with no
  rollback of the acquired BO;
- the fence-ticket failure three lines below **does** release its own pool slot,
  so the rollback pattern exists at the site and that branch simply does not
  follow it;
- the audit and other fallible paths after acquisition were reported as lacking
  BO rollback as well, and the fix must cover the whole family, not one branch.

A BO left in `Recording` is never selected again — selection accepts only
`Free` — and it carries no owner buffer and no completion waiter, so nothing
else will ever move it. The slot is lost for the life of the pool.

## Evidence, sixth hardware run (card1 / HDMI-2, cross-device copied route)

```text
retry_skip_counts=[("PendingAcks",0), ("RetryDeadline",0), ("EmptyDamage",0),
                   ("NoBO",1441), ("NoPool",1), ("NothingPending",0)]
descriptor_pool_slots_in_use=3/3
bo_idx=2 phase=Recording destination_releasable=Some(true)
  owner_ledger_state=None owner_ledger_current=false
  acquired_then_skipped_same_tick=true
  acquisition_skip_events=[{ tick: 1, bo_idx: 2, generation: 5, reason: "NoPool" }]
render_completion_waiters=[]
```

One `NoPool` stranded `bo_idx=2`; the following **1441** ticks all skipped with
`NoBO` until the deadline. The acquisition and the skip are tied to the same
tick id, so the pairing is observed, not inferred.

## Severity, and why it is ours as well

The trigger in that run was our own descriptor-slot leak (documented in plan Cp;
`OwnerBuffer::into_submitted` loses the slot), which guarantees ring exhaustion
after three composed Owner frames. **Fixing our leak does not fix this.** It
converts a certain stall into an occasional one: any transient exhaustion — a GPU
backlog, more outputs, a deeper pipeline — will still strand a buffer
permanently, and an intermittent permanent stall is harder to diagnose than a
deterministic one.

It also reaches production today through the Legacy route, independently of C.0.

## Why we took responsibility, and the rule this sets for stages 3 and 4

The user's criterion (2026-09-22): **"si nos afecta, también es responsabilidad
nuestra"** — upstream authorship decides who *wrote* a defect, not who must fix
it. What decides ours is whether it touches what we are building. This one met
three tests, and future cases should be judged by the same three:

1. **It bites our route now.** It stranded a destination on the Owner copied
   route during the cross-device hardware run. Not hypothetical, observed.
2. **It keeps biting after our own fix.** Our descriptor-slot leak made the
   exhaustion certain; removing it makes the exhaustion occasional, and every
   occasional exhaustion still strands a buffer permanently. Fixing only our
   half would have converted a deterministic failure into an intermittent one —
   strictly harder to diagnose, and easy to mistake for flakiness later.
3. **It reaches production through the shared path.** The tick is common to
   Legacy and Owner, so the defect ships today, independently of C.0.

Contrast with the Legacy dormancy bug (`walked()`, Jos, `6e1ba09b`), which was
left to upstream: it fails none of the three. It does not touch the converted
routes, our work neither triggers nor masks it, and nothing we are building
depends on it. That is the line.

**Why this matters for the rest of C.0.** Stages 3 and 4 convert lifecycle,
modeset, DPMS, VT, topology, cursor and gamma — paths that are shared with the
Legacy route and largely upstream-authored, far more so than the producers
stage 2 converted. Defects of exactly this shape will surface again, and the
default answer should not be re-argued each time:

- a defect in upstream code that the conversion **reaches** is ours to fix,
  in its own commit, portable upstream as its own PR, and named as an addendum
  rather than as stage scope;
- a defect in upstream code the conversion **does not reach** is reported with
  its reproduction and left upstream;
- the distinction is made on the three tests above, not on who wrote the file.

The cost asymmetry is what justifies the default. Fixing an upstream defect we
reach costs one small commit. Not fixing it costs an intermittent stall inside a
route we are simultaneously rewriting, where every future failure has two
candidate explanations instead of one — and stages 3 and 4 are where that
ambiguity would be most expensive, because their failures are lifecycle
failures: a device that will not light, a VT that will not come back.

## Disposition

1. Fix it in this branch, covering **every** fallible return between acquisition
   and render submission, not only `NoPool` — the sibling-site lesson this
   session has already paid for twice.
2. Keep it in its own commit so it can be sent upstream as its own PR, as was
   done for the accel-profile width bug (#116) and the GLX defects (#118-#120).
3. The upstream report carries this document's evidence block verbatim.

## Ready-to-file upstream issue text

> **The composite tick can strand a scanout BO when a step after acquisition
> skips the frame**
>
> `acquire_scanout_bo` / `acquire_managed_scanout_bo` transition the selected
> destination from `Free` to `Recording` and the tick owns that transition
> (B-13). Some fallible steps that run after it return early without restoring
> the phase — the descriptor-pool exhaustion branch is the one we observed,
> returning `Skipped(NoPool)` — while the fence-ticket failure path a few lines
> below does release its own resource.
>
> Because selection only ever accepts `Free` buffers, a destination left in
> `Recording` is never chosen again and nothing else moves it: no owner buffer,
> no completion waiter. A transient shortage therefore becomes a permanent
> one-buffer-smaller pool.
>
> Observed on a cross-device (PRIME) copied scanout route on real hardware: a
> single `NoPool` skip stranded one of three destinations, after which 1441
> consecutive ticks skipped with `NoBO` until the test deadline. The acquisition
> and the skip were recorded with the same tick id.
>
> Suggested fix: restore the acquired BO's phase on every fallible return
> between acquisition and render submission, following the pattern the
> fence-ticket path already uses for its own resource.
