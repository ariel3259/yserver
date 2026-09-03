# Deadline-scheduled primary-plane submission — low-latency synchronized scanout

**Date:** 2026-08-31
**Status:** Draft for review
**Baseline:** `master` at `c09358a1`, including merged Phase A+B at `fc76b743`
**Suggested branch:** `feat/deadline-scheduled-primary-plane-submission`
**Roadmap:** Standalone; it has no dependency on another roadmap design

## 1. Summary

The merged fullscreen direct path keeps one hardware flip in flight and one
bounded latest-wins successor. When the predecessor retires at vblank `N`, it
submits the successor immediately; that successor can first become visible at
vblank `N+1`. This is throughput-safe and preserves buffer ordering, but it
chooses the displayed frame a full refresh period before scanout.

This design retains the successor until a measured deadline shortly before
vblank `N+1`. New eligible frames may replace it during that retention window,
so the frame scanned at `N+1` is newer. The policy applies only to synchronized
direct primary-plane replacement. It preserves one flip in flight, bounded
latest-wins storage, immediate release of displaced never-submitted buffers,
and predecessor-before-`Skip` completion ordering.

The feature is autonomous. It extends the merged `ScanoutM2State` and direct
retirement path directly. It neither requires nor prepares any other roadmap
work.

## 2. Problem and baseline

The merged path is:

```text
vblank N
  -> page-flip event
  -> retire_direct_output
  -> enqueue predecessor completion and deferred Skips for ordered publication
  -> submit_queued_direct_successor
  -> atomic_commit(PAGE_FLIP_EVENT | NONBLOCK)
  -> core publishes the queued protocol events
vblank N+1
  -> successor becomes visible
```

For a refresh period `T`, a frame selected at retirement is approximately one
period old before it can be scanned. With a 200 fps producer at 60 Hz, scanout
age is normally about 16.7–21.7 ms; with a 60 fps producer it may approach
16.7–33.3 ms. Latest-wins selects the freshest frame available at retirement,
but cannot remove the following full-period wait.

The existing behavior is normative in every other respect:

- core Present supersession requires CRTC, target MSC, and coverage
  equivalence;
- direct scanout has at most one submitted frame and one queued successor;
- a displaced queued buffer idles and releases immediately;
- its `Skip` completion is deferred behind the submitted predecessor;
- unflip, cursor/topology invalidation, VT, DPMS, hotplug, and shutdown can
  cancel direct ownership; and
- no runtime gating environment variable is acceptable.

## 3. Goals

1. Remove most of the dispatch-to-display refresh period for synchronized
   direct scanout.
2. Preserve exactly one submitted direct flip and one latest-wins queued frame.
3. Preserve immediate one-shot buffer idle/release and ordered deferred `Skip`.
4. Derive the dispatch margin from measured behavior for the current DRM
   device, topology, and mode; expose no tuning control.
5. Detect a missed target and fall back toward the known-safe immediate policy
   without a retry loop or unbounded queue.
6. Preserve direct unflip and every lifecycle invalidation rule.
7. Validate latency and judder together; a latency win cannot hide missed
   flips or uneven frame delivery.

## 4. Non-goals

- Async page flips, tearing, `PAGE_FLIP_ASYNC`, or Present capability changes.
- Cursor movement, cursor-plane policy, gamma, color management, or a generic
  KMS commit owner.
- Variable refresh rate.
- Changing core Present target calculation, equivalence, or parking.
- Scheduling composed frames or normal desktop updates.
- Guaranteeing a particular application FPS.
- An environment variable, CLI option, config key, driver allowlist, or manual
  millisecond setting for enablement or margin.

## 5. Eligibility

Deadline retention is eligible only when all of these are true:

```text
candidate passes the complete merged direct-scanout predicate
&& (candidate.masked_options & PRESENT_ALL_ASYNC_OPTIONS) == 0
&& direct ownership is already active
&& no direct flip is currently pending after predecessor retirement
&& the output group has one effective refresh period
&& the selected completion CRTC has a current monotonic clock sample
&& device/topology/CRTC clock epochs match the retained schedule
&& no unflip, lifecycle, topology, cursor-policy, or recovery invalidation exists
```

The existing grouped direct path already requires one DRM device and matching
effective refresh across its output set. The selected completion CRTC supplies
the schedule clock for the whole grouped transaction. When the complete merged
direct predicate and ownership/lifecycle validity still hold, an unknown
period, invalid timestamp conversion, stale schedule identity, or another
retention-only failure uses immediate submission for the newest successor; it
does not reject direct scanout. Failure of the merged direct predicate or an
unflip, lifecycle, topology, cursor-policy, or recovery invalidation instead
cancels retention and enters the existing unflip/fallback teardown without
submitting that candidate.

The first direct entry remains immediate. Scheduling begins only after a real
direct predecessor has retired and established the next target interval.

For this design, a **synchronized candidate** is one whose masked Present
options contain neither `PRESENT_OPTION_ASYNC` nor
`PRESENT_OPTION_ASYNC_MAY_TEAR`, expressed by the complete
`PRESENT_ALL_ASYNC_OPTIONS` mask above. A **compatible replacement** is a newer
synchronized candidate that passes the same complete-plane direct predicate
for the retained physical primary plane, output group, device incarnation,
topology identity, completion CRTC epoch, and scheduled target interval.

The merged successor remains generic, so mixed streams use this normative
arbitration:

| Window state | New synchronized candidate | New async-option candidate |
| --- | --- | --- |
| No window/pending predecessor | Existing generic latest-wins queue behavior. | Existing generic latest-wins queue behavior. |
| `OpenWindow` | Become the sole retained successor when compatible; otherwise cancel the window and follow merged immediate admission. | Cancel the window and submit through the merged immediate path. |
| `Retained` | Compatibly replace the retained successor. | Replace the retained successor through the same generic slot, cancel retention, and submit immediately through the merged path. |

Every replacement in this table preserves the merged immediate one-shot
Idle/release and deferred-`Skip` contract. An async-option candidate is never
held by this scheduler and never trains its estimator. There is still only one
generic successor slot; cancellation/bypass creates neither a second queue nor
overlapping submission.

## 6. State model

`ScanoutM2State` gains one bounded schedule:

```rust
struct DirectDispatchWindow {
    generation: u64,
    device_key: DrmDeviceKey,
    device_incarnation: u64,
    master_epoch: u64,
    dispatch_topology_generation: u64,
    crtc_key: CrtcKey,
    crtc_epoch: u64,
    target_msc: u64,
    target_ust: u64,
    deadline: Instant,
    margin: Duration,
}
```

The existing `queued_successor: Option<DirectPresentFrame>` remains the only
unsent framebuffer slot. The window owns no framebuffer and creates no second
queue. Replacing the successor leaves the window target unchanged and follows
the existing immediate-idle/deferred-`Skip` split.

The feature owns local checked identities rather than assuming an unmerged
topology facility. `device_incarnation` is allocated whenever the DRM fd family
is opened or replaced and is retired on device loss or reopen. `master_epoch`
is incremented before every DRM-master authority transition, including loss and
reacquisition on the same fd, so no timer crosses a VT/master boundary. One
device-incarnation-wide monotonic allocator supplies a fresh
`dispatch_topology_generation` for every direct-group creation and before any
change to connector/CRTC routing, output membership, mode timing, selected
completion CRTC, or connector class becomes visible. Destroying and recreating
a group therefore cannot reuse a generation within the incarnation. A device
reincarnation starts fresh master/topology namespaces. Dispatch topology
identity describes physical schedule shape; the merged CRTC clock epoch
independently describes timestamp continuity, and all identities must match.
Exhausting any checked namespace disables scheduling for that incarnation and
selects immediate behavior rather than wrapping.

The complete states and transitions are:

```text
Inactive
  -> Pending(frame)                    on first direct entry (immediate submit)

Pending(frame submitted)
  -> Retained(window, successor)       on retirement with eligible sync queue
  -> OpenWindow(window)                on retirement with no queue
  -> Pending(promoted successor)       on retirement with async or
                                         scheduling-ineligible/direct-valid queue
  -> Inactive/unflip                   on invalidation

Retained
  -> Retained(newest successor)       on compatible replacement
  -> Pending(async successor)         on async-option arrival; cancel window
  -> Pending(successor)               on scheduling-eligibility loss while
                                         direct/lifecycle validity remains
  -> Inactive/unflip                  on invalidation
  -> Pending(successor)               at deadline or late arrival

OpenWindow
  -> Retained(window, successor)      on compatible synchronized arrival
  -> Pending(successor)               on async, late, or scheduling-ineligible
                                         but direct-valid arrival
  -> Inactive                         on empty expiry
  -> Inactive/unflip                  on invalidation
```

Here **scheduling-ineligible/direct-valid** means the candidate still satisfies
the complete merged direct predicate and current ownership/lifecycle rules but
fails only this feature's retention conditions, such as usable schedule clock,
single effective period, current schedule identity, or an unexpired window. It
uses immediate merged submission. Loss of direct ownership or the merged direct
predicate, or an unflip, lifecycle, topology, cursor-policy, or recovery
invalidation, is never classified that way: it cancels the window and enters
the existing unflip/fallback teardown without submitting the candidate.

`OpenWindow` permits a compatible frame arriving before the deadline to become
the sole retained successor. If no frame exists when the deadline fires, the
window expires without a commit. A frame arriving after the deadline submits
immediately through the existing path.

Every timer callback carries `generation`; a callback whose generation, device
incarnation, master epoch, dispatch topology generation, CRTC epoch, target, or
direct-ownership state is stale is a no-op. A superseded or expired callback
never cancels a newer window. Generation allocation is checked and never wraps
silently.

## 7. Deadline construction

At predecessor retirement, let:

- `event_msc` be the validated 64-bit media-stream count;
- `event_ust` be the validated DRM monotonic timestamp in integer microseconds;
- `T` be the unit-typed exact effective mode period and `T_us` its checked
  integer-microsecond representation;
- `target_msc = event_msc + 1` with existing wrap-safe MSC arithmetic;
- `target_ust = event_ust + T_us` in integer microseconds with checked
  monotonic arithmetic; and
- `margin` be the current estimator output from section 8.

Then:

```text
margin_us = checked integer-microsecond representation of margin
deadline_ust = target_ust - margin_us
deadline = monotonic_now + max(0, deadline_ust - monotonic_now_ust)
```

`monotonic_now_ust` is the same receipt sample expressed in integer
microseconds; only the final checked delta is converted to the unit-typed
`Instant`/`Duration` timer API.

The implementation samples `CLOCK_MONOTONIC` at event receipt and uses the
same clock domain as DRM monotonic event timestamps. Delivery delay therefore
reduces the remaining retention window rather than moving the target. If the
deadline has passed, promotion is immediate.

Deadline firing revalidates the complete direct predicate and all identities,
then submits at most the newest queued successor. A newer invalidation always
wins. The timer never calls the ioctl from a signal handler; it wakes the normal
backend event loop and uses the existing direct submit path.

## 8. Adaptive margin

The estimator is keyed by `(device incarnation, master epoch, dispatch topology
generation, effective mode period)`. It stores a bounded ring of the last 256
successful direct submissions:

```text
host_cost = ioctl_result_monotonic - dispatch_monotonic
on_time   = normalized_retired_msc == scheduled_target_msc
```

It never mixes device incarnations, dispatch topology generations, refresh
periods, rejected calls, stale events, async-option Presents, async flips, or
composed/unflip transactions.

The exact policy is:

1. A new key starts in `Conservative` with `margin=T`, which is behaviorally
   equivalent to immediate submission.
2. After 32 consecutive successful on-time samples, compute the nearest-rank
   p99 host cost over the current ring and set the desired floor to
   `candidate = clamp(p99 + 1 ms, 1 ms, T)`.
3. Actual margin may decrease by at most 250 microseconds at each completed
   block of 32 consecutive on-time samples, including the first qualifying
   block. It moves toward `candidate`, never jumps directly to it and never
   falls below it.
4. One missed target immediately sets
   `margin = min(T, max(2 * margin, host_cost + 2 ms))`, clears the success run,
   and records a miss. Two misses within any 120 scheduled submissions return
   the key to `Conservative` for the next 120 successful submissions.
5. Rejection, missing/malformed completion, lifecycle invalidation, or clock
   uncertainty does not train the estimator and selects the existing safe
   recovery/fallback.

All arithmetic is checked. A ring or timestamp failure resets only this policy
key to `Conservative`; it never changes direct correctness or capability.

Estimator state is destroyed on DRM fd reopen or replacement, device/master
loss, driver reset/recovery, CRTC clock-source or clock-epoch replacement, and
server restart, even if stable device identity and mode timing are unchanged.
A new device incarnation or clock source always requalifies conservatively
against the current driver/kernel state.

## 9. Ordering and invalidation

The schedule changes the dispatch instant, not ownership rules:

- retirement enqueues predecessor completion before deferred successor `Skip`s
  for ordered later publication, exactly as in the merged path; owner dispatch
  may occur before core publishes that queue, but client-visible order cannot;
- a retained successor is never reported as submitted or presented;
- replacement immediately idles/releases only the displaced never-submitted
  frame and never the current or pending frame;
- unflip request, cursor/output eligibility loss, dispatch topology generation
  change, VT release, DPMS-off, hotplug, device loss, shutdown, or direct-submit
  rejection cancels the window before running existing teardown;
- a cancellation terminalizes the retained frame through existing direct
  `Skip`/fallback semantics exactly once;
- no timer survives a CRTC clock-epoch change; and
- failed promotion requests the existing safe direct unflip and cannot arm a
  second timer for the failed generation.

Submission still occurs only after the predecessor has retired. Retaining the
successor within that already-safe interval does not permit overlapping flips.

## 10. Telemetry

Record per policy key:

- window generation, target MSC/UST, deadline, margin, and eligibility reason;
- retirement receipt delay and remaining retention interval;
- queued replacements, immediate idles/releases, deferred `Skip`s, and
  duplicate-prevention counters;
- dispatch, ioctl result, and page-event timestamps;
- host-cost ring occupancy and p50/p95/p99;
- estimator state, margin increases/decreases, conservative resets, and reason;
- scheduled target versus retired MSC, on-time count and missed-target count;
- cancellations classified by unflip, cursor eligibility, topology, VT, DPMS,
  hotplug, device loss, shutdown, stale timer, or submit failure;
- Present-to-dispatch and Present-to-scanout age p50/p95/p99; and
- flip-interval/judder distribution, including intervals over 1.5T and 2T.

Normal per-frame detail is debug/trace only. Aggregates are available without
introducing a production behavior switch.

## 11. Verification

### 11.1. Unit and state-machine tests

1. First entry and an unqualified/unknown-clock key submit immediately.
2. Retirement constructs the exact next MSC/UST and checked deadline.
3. Multiple compatible arrivals retain only the newest framebuffer while every
   displaced buffer idles/releases once and its `Skip` remains ordered.
4. A deadline submits exactly once through the normal event loop.
5. An arrival after the deadline submits immediately.
6. An empty open window expires without a commit; a later frame uses normal
   admission.
7. Every identity/epoch mismatch makes a stale callback a no-op.
8. Unflip and every lifecycle invalidation cancel before promotion and preserve
   current/pending framebuffer lifetimes.
9. The 256-sample ring, nearest-rank p99, clamps and checked arithmetic match
   fixed vectors.
10. Fewer than 32 samples remains conservative; decrease is bounded to 250 us
    per 32 successes.
11. One miss expands margin immediately; two misses in 120 enter the exact
    conservative cooldown; successes exit it only after 120 samples.
12. Rejection, malformed event and composed/async samples never train the key.
13. Device incarnation, same-fd master loss/reacquisition, DRM reopen, driver
    recovery, group destroy/recreate, topology, completion-clock epoch, and mode
    changes isolate or reset estimator state and stale timers cannot alias a new
    group or master epoch.
14. Target-`u64` and monotonic-time overflow fail safe to immediate behavior.
15. No environment, CLI, config or driver-name branch controls the feature or
    margin.
16. An async-option arrival in `OpenWindow` or `Retained` cancels retention,
    follows the generic latest-wins Idle/deferred-`Skip` contract, submits by
    the merged immediate path, and never trains the estimator.
17. Every `Pending` retirement and every `OpenWindow` arrival, expiry,
    invalidation, generation-exhaustion, and stale-callback branch follows the
    transition table exactly once.
18. Scheduling-only ineligibility falls back to immediate direct submission,
    while loss of direct/lifecycle validity cancels into existing
    unflip/fallback teardown and never submits the candidate.

### 11.2. Hardware validation

At minimum validate NVIDIA proprietary and AMDGPU/RADV at 60 Hz and the highest
available fixed refresh, with 60 fps and uncapped 200+ fps producers:

- baseline immediate submission versus scheduled submission using identical
  builds except for the reviewed change;
- fullscreen entry, steady direct ownership, continuous replacements,
  Alt-Tab/unflip/re-entry, cursor movement, VT, DPMS, hotplug, and shutdown;
- forced late event-loop wakeups and injected ioctl latency proving adaptive
  fallback rather than repeated misses;
- a cold estimator, converged estimator, one miss, two-miss cooldown, topology
  reset, and server restart; and
- trace correlation proving one in-flight flip, one queued frame, immediate
  displaced-buffer release, ordered completion, and no stale timer submit.

For each run record Present-to-scanout age, dispatch margin, retired target,
missed targets, flip intervals, application FPS, Complete/Idle/Skip rates,
buffer high-water, GPU load, and subjective judder.

## 12. Acceptance criteria

The design is complete when:

- median Present-to-scanout age falls by at least `0.5T` for the uncapped direct
  workload after estimator convergence;
- p95 age improves without increasing p99 flip interval by more than 5% against
  the immediate baseline;
- the converged run has zero missed targets in a continuous 10-minute sample;
- forced misses expand/reset the margin exactly as specified and recover
  without a restart;
- no run exceeds one submitted direct flip or one queued successor;
- every displaced never-submitted buffer idles/releases immediately and once,
  while its `Skip` follows the predecessor completion;
- synchronized tear-free behavior, unflip and lifecycle correctness remain
  unchanged;
- no production-reachable tuning or enable lever exists; and
- `cargo +nightly fmt -- --check`,
  `cargo clippy --all-targets -- -D warnings`, and
  `cargo test --all-targets --locked` pass.

Failure of the latency or judder gate means the standalone PR does not merge,
so `master` keeps its immediate policy. It does not weaken the thresholds,
ship a disabled alternate path, or expose a manual override.

## 13. Implementation boundary

This is one standalone PR based on the then-current `master`. It contains the
bounded window, timer wake, estimator, invalidation wiring, telemetry and tests
as one reviewable behavior. It does not modify or depend on another design's
documents, branches, readiness gates, or implementation plans.

## 14. References

- Merged direct successor implementation at `fc76b743`
- Follow-up VT cursor-plane fix at `c09358a1` as the reviewed master baseline
- `joske/yserver#129` maintainer requirement: one flip in flight, one bounded
  coalesced successor, submit only after predecessor retirement
- Xorg Present flip-ready ordering in `present_scmd.c`
- wlroots frame-pending/direct-scanout ordering model
- DRM monotonic event timestamps and page-flip event MSC/UST semantics
- Adversarial source analysis preserved at
  `docs/superpowers/findings/2026-08-31-phase-c0-spec-vs-merged-phase-ab.md`;
  this is provenance only, not a normative dependency
