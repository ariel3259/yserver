# The compose tick strands a scanout BO on any post-acquisition skip

**Status:** confirmed on hardware 2026-09-22 during plan Cp task 8's
cross-device run. **FIXED in the 2026-09-22 C.0 follow-up.** The implementer
F8'd on this document's original Legacy claim; coordinator verification
confirmed that only managed acquisition transitions `Free -> Recording`, so
the defect is ours and there is no upstream PR to send. The original
classification is kept below with the correction, because the mistake is
instructive.

**Where:** the managed acquisition in `crates/yserver/src/kms/render/platform.rs`
and the early returns of the composite tick in `scene.rs`.

**The correction (implementer F8, verified by the coordinator).** This document
first claimed the defect reaches production through the Legacy route, because
the tick is shared. That is false. `acquire_scanout_bo`, the Legacy
acquisition (`platform.rs:6423`), only *selects* a bo whose phase is `Free` and
returns a token — **it does not transition the phase**. Legacy's transitions
happen inside the render helpers, immediately before rendering
(`scene.rs:10208` shared, `:10531` copied), so a Legacy skip cannot strand
anything: the bo stays `Free` and the next tick takes it again.

Only `acquire_managed_scanout_bo` transitions `Free -> Recording` at
acquisition (`platform.rs:6744`), and that transition is **ours** — B-13, from
stage 2c-i, whose comment says it exists so "legacy `acquire_scanout_bo` can
[not] hand out the same index while this managed token is live". So we
introduced a phase whose lifetime spans code paths that were never written to
roll it back. Upstream's skip branches are unchanged and correct for their own
route; there is nothing to file upstream.

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

## Why this is ours, and what the criterion still gives stages 3 and 4

The user's criterion (2026-09-22) — **"si nos afecta, también es
responsabilidad nuestra"** — was written here against three tests. This case
now meets them differently than first recorded, and the difference is the
lesson:

1. **It bites our route now.** Still true: it stranded a destination on the
   Owner copied route during the cross-device hardware run.
2. **It keeps biting after our own related fix.** Still true: the
   descriptor-slot fix makes exhaustion occasional rather than certain, and
   every occasional exhaustion still strands a buffer permanently — a
   deterministic failure turned into an intermittent one.
3. **~~It reaches production through the shared path.~~ FALSE.** Legacy never
   transitions at acquisition, so Legacy cannot strand. This defect lives only
   on the managed route.

Test 3 was the one that made it look like upstream's problem to fix. With it
gone, ownership is simpler, not harder: **the hazard is created by our own
B-13 transition**, so the defect is ours by authorship as well as by reach, and
the "addendum" framing does not apply — this is C.0 scope.

**What stages 3 and 4 should take from it.** The criterion stands for future
cases, and so does the contrast with the Legacy dormancy bug (`walked()`, Jos,
`6e1ba09b`), which meets none of the three and was rightly left upstream. But
the process lesson is sharper than the rule: **this document asserted a
mechanism across a code path it had not read, and the implementer caught it by
refusing to code against an assumption that did not hold.** Stages 3 and 4
convert lifecycle, modeset, DPMS, VT and topology — paths far more entangled
with Legacy than stage 2's producers — so the temptation to reason "the tick is
shared, therefore Legacy is affected" will recur constantly. Read the Legacy
arm before asserting anything about it; being wrong in the safe direction
(claiming upstream owns something we own) still costs a round trip and would
have produced a filed issue that upstream would have had to reject.

## Disposition and implementation

The managed destination is cancelled with the existing
`cancel_scanout_bo_recording` helper at the four reachable pre-submit exits:
the audit overlay pipeline error, damage-audit error, descriptor `NoPool`, and
fence-ticket error. The fence-ticket branch also releases its already-acquired
descriptor slot; that cleanup remains in place. The temporary acquisition
leases have already been dropped, the pool registration stays installed, and
the acquired scanout BO has not been submitted and has no completion waiter at
these exits.

The other eight enumerated sites do not need rollback code. The post-acquire
DRM-device and scanout-pool lookups are guaranteed by the successful managed
acquisition and no synchronous tick step replaces either. The shared BO index
was selected from that same pool. The later XOR-cache lookup either is skipped
with an empty overlay or hits the exact cache entry created by the earlier
audit-pipeline lookup; if that earlier lookup fails, the tick has already
returned. Both missing-service checks are unreachable because a successful
managed acquisition required the still-present `Option` service. The copied
source and destination lookups are only in the Legacy `else` arm; managed
copied output takes the Owner arm instead.

Tests, all named `c0_conv_cp_..._vulkan` and ignored with
`needs live Vulkan ICD`, force each reachable site and assert the acquired
destination returns to `Free`, its managed registration remains, the temporary
lease is gone, and no Owner membership, page-flip, completion waiter, or extra
descriptor slot remains. The pipeline, `NoPool`, and fence tests use the copied
Owner fixture. The damage-audit error test uses the managed Shared fixture
because the audit routine deliberately skips copied outputs. Four independent
remove-the-rollback mutations, each compiled and run against its matching test,
failed at `Recording` versus `Free` and were reverted.

The requested debug/release build, formatting, clippy, C.0 filter, and full
library test gate passed. No hardware `_drm` test, modeset, or DRM-master path
was run. **No upstream PR.**

## Withdrawn: the upstream issue text

An issue text was drafted here while the defect was believed to reach Legacy.
It is withdrawn — Legacy does not transition at acquisition and has no defect
to report. Kept only as the record of a claim this document made and corrected.
