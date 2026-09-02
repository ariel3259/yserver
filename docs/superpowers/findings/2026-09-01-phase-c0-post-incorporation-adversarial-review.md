# Phase C.0 post-incorporation signaling and returnability review

**Date:** 2026-09-01  
**Reviewer:** Opus  
**Reviewed spec:** the merged-code-baseline-revised Draft of
`2026-08-26-phase-c0-atomic-kms-migration-design.md`  
**Kernel basis:** local `~/Projects/linux` at
`77cb8f24c2381a8abb7272d7bbdec548d6426a8a`  
**Disposition:** Incorporated after maintainer/user approval, with one factual
qualification

## Verified kernel behavior

`prepare_signaling()` in `drm_atomic_uapi.c` assigns
`drm_crtc_state.event` when either the atomic request has
`DRM_MODE_PAGE_FLIP_EVENT` or that CRTC has `OUT_FENCE_PTR`. The subsequent
atomic CRTC check rejects the request when both old and new CRTC states are
inactive. The restriction is therefore about kernel signaling event state, not
only a page event delivered to userspace.

`drm_crtc_get_sequence_ioctl()` and `drm_crtc_queue_sequence_ioctl()` both
return `EOPNOTSUPP` when `drm_dev_has_vblank()` is false. The generic
`drm_crtc_send_vblank_event()` path under the same condition emits raw sequence
zero with a monotonic `ktime_get()` timestamp. These generic outcomes share one
core condition and are not independent qualification evidence.

The review's claim that raw zero is never written by the Asahi DCP path was not
true for the audited tree. Commit `d15af95c52ec` adds
`dcp_crtc_send_page_flip_event()`, bypasses the generic sender for flip-complete,
and explicitly sets `seq = 0` with a DCP-adjusted monotonic timestamp. The spec
now documents both the generic Linux rule and this driver override. Runtime
classification remains independent of either event value: only a current
GET_SEQUENCE `EOPNOTSUPP` selects `FlipDrivenSoftware`, where all later raw page-
event sequences are ignored.

`create_vblank_event()` initializes `event.vbl.crtc_id` from
`crtc->base.id` unconditionally. A zero CRTC id paired with a current tagged
event therefore remains a direct completion-mechanism contradiction.

## Accepted corrections

1. **Off-to-off signaling.** Section 6.3, unit test 53, physical validation,
   acceptance, and references now reject an old-inactive/new-inactive CRTC when
   either the global page-event flag or a local out-fence pointer creates kernel
   event state. Off-to-off is permissible only with neither signaling source;
   adding an out-fence merely for symmetry is forbidden and tested.
2. **Independent returnability evidence (initial fixed deadline; superseded by
   item 20).** The provisional in-process campaign
   adds a separate externally supervised eight-hour returnability arm on both
   required devices. Durable begin/end records and a two-second seat-active
   deadline distinguish latency from an unreturned call. Cold/offline calls use
   a reported, non-selecting 30-second deadline.
3. **Explicit selection reason.** Passing every latency row with zero deadline
   misses selects `InProcessQualified`; a deadline miss selects
   `ExecutorReturnabilityRequired`; a latency failure with zero misses selects
   `ExecutorResponsivenessOnly`. The latter must not be presented as evidence
   that an ioctl failed to return.
4. **No thread-executor architecture.** Selection remains globally binary.
   A worker thread can hide a slow return from the X11 core, but an unexpected
   late/non-returning call still retains holder memory, possible accepted KMS
   state, incarnation ownership, and quarantine until actual return. Supporting
   it would require a thread-stall/join barrier while losing process crash
   containment, so it does not replace the existing completion-unknown design.
5. **No circular soak.** Each provisional returnability arm reaches eight hours
   or terminates with a durably recorded seat-active miss before architecture
   selection. The final eight-hour soak is a distinct run against the selected
   architecture and validates rather than selects it.

## Measurement-validity follow-up

Opus accepted the separation of latency and returnability evidence, then found
four additional ways in which the proposed measurement mechanics could still
predetermine or contaminate the result. The maintainer/user approved the
separation and the four follow-up corrections below:

6. **Separate artifacts from one source commit.** The latency/concurrency
   artifact performs no supervisor IPC, filesystem I/O, allocation, or flush on
   a measured path and records into a fixed-size preallocated buffer. The
   returnability artifact alone uses the synchronous supervisor protocol. Its
   measured durations are characterization and cannot pass or fail the latency
   bounds.
7. **Non-self-extinguishing absorbed-cursor shape.** A third documented,
   nonmergeable measurement diff suppresses transport closure only for the
   deliberately injected unchanged-cursor-absorbed conflict. It records the
   production closure that would have occurred and preserves coordinate
   coalescing plus the single deferred retry after the matching primary
   completion. It does not relax production section 7.1 behavior.
8. **Corroborated returnability.** `HostCallBegin` carries timestamps on both
   sides and its acknowledgement must complete within the predeclared 10 ms
   `SupervisorAckBound`; otherwise the sample is `InstrumentRejected` and does
   not enter the ioctl. After a valid acknowledgement, the target stores a
   syscall-free `HostCallEntered` marker in supervisor-owned shared memory. A
   deadline miss counts as non-return only when the marker exists, no return is
   recorded, and `/proc/<pid>/task/<tid>/syscall` confirms the expected ioctl,
   fd, and request. `wchan` and stack are collected when available.
9. **Fourth gate outcome.** Missing phase coverage, buffer overflow, rejected
   instrumentation, or absent syscall corroboration yields
   `EvidenceInsufficient`. It selects neither architecture, keeps the spec
   Draft, and requires a rerun with a workload and artifact that satisfy the
   predeclared evidence contract.
10. **Phase and retry coverage.** For each required device and each combination
    of composed/direct traffic with cursor-omitted/unchanged-cursor-absorbed
    shape, the arm requires 100,000 qualified initial attempts between accepted
    primary dispatch and `HardwareComplete`, with at least 5,000 samples in each
    of ten normalized phase deciles. Retries and non-overlap samples do not fill
    those quotas. Each retry links to its initial `EBUSY` and must occur after
    the matching primary completion but before the next primary admission; an
    ordering violation is a normative failure. Buffer capacity uses checked
    arithmetic for the complete arm, and overflow is never wrapped or silently
    overwritten.

## Phase-targeting follow-up

The next Opus pass showed that the preceding absorbed shape could produce only
one initial attempt per accepted-primary cycle and that ordinary 1000 Hz input
would place it near dispatch. Its deferred retry occurs after
`HardwareComplete` and cannot fill later phase deciles. The maintainer/user
approved these corrections:

11. **Targeted absorbed-arm sampling.** The same nonmergeable absorbed-arm diff
    phase-targets its sole initial attempt. After 64 canonical intervals from
    the same incarnation, CRTC clock epoch, mode, and traffic class, it uses
    their median to arm a decile-center timer. Target deciles rotate in balanced
    order independently of result and errno. Input remains latest-wins coalesced
    until the timer; missing new input, early completion, or a lifecycle barrier
    produces no ioctl. At most one initial attempt occurs in a primary cycle,
    and `EBUSY` retains only the production deferred retry after completion.
12. **Actual-phase evidence.** Records retain predictor inputs, target and actual
    deciles, timer disposition, and attempts per cycle. Only the actual phase in
    the eventual dispatch-to-`HardwareComplete` interval fills a quota. Warm-up,
    cancellation, non-overlap, and retry samples cannot pad it; identity or mode
    replacement resets the predictor. Each stratum is bounded by 250,000
    accepted-primary cycles and 250,000 initial attempts after warm-up; hitting
    either cap without full coverage is `EvidenceInsufficient` and keeps the
    fixed-buffer capacity finite.
13. **Gate terminology.** The evidence gate has four outcomes, including the
    non-selecting `EvidenceInsufficient`. Only a conclusive result reaches the
    still-binary choice between in-process execution and `KmsIoExecutor`.

## Weighted phase-coverage follow-up

The following Opus pass identified that target decile 9 has no higher target
whose longer-than-predicted interval can spill down into actual decile 9. Lower
targets can still spill upward when an interval is shorter than predicted, so
the review's absolute claim that only target 9 can feed actual decile 9 was too
strong. The actionable point remains: a balanced 25,000-target allocation can
underfeed the top actual decile under jitter and exhaust the stratum cap. The
maintainer/user approved a weighting that gives every decile a fixed base and
then compensates for its measured same-decile yield without relying on spillover:

14. **Frozen weighted allocation (initial estimator length; superseded by item
    17).** The 64 warm-up intervals project every
    decile-center offset and count `WarmupSelfHits[d]`. Every decile receives
    10,000 base targets; the remaining 150,000 are apportioned by
    `64 / max(1, WarmupSelfHits[d])`, exact floors, largest fractional remainder,
    and higher-decile tie-break. The resulting budgets total 250,000 and freeze
    before the first coordinate attempt.
15. **Result-independent order.** A checked weighted-fair credit rotation
    interleaves the frozen budgets. Every accepted-primary cycle consumes its
    chosen target even if early completion, missing input, or lifecycle
    cancellation prevents the ioctl. Results, errno, observed coverage, and
    interval duration cannot alter later targets. Only an identity, epoch, mode,
    or traffic-class replacement discards the schedule and repeats warm-up.
16. **Operational budget.** At 60 Hz the 250,000-cycle cap is about 69 minutes
    27 seconds per stratum and four strata are about 4 hours 38 minutes per
    device. Adding the eight-hour returnability arm and final eight-hour soak is
    about 20 hours 38 minutes per device, or 41 hours 16 minutes of dedicated
    device time across both required machines, excluding setup and the remaining
    validation matrix. Parallel machines reduce wall time but not device time;
    this estimate does not relax any gate.

## Weight precision and required-AMD follow-up

The next Opus pass accepted the weighted scheduler but found that freezing its
weights from 64 observations made rare self-hit estimates unstable. It also
found that the normative hardware matrix still named the maintainer's retired
RX 580. The maintainer/user approved:

17. **Separate estimator length.** `PredictorWindow` remains 64. After 64 cycles
    prime that rolling predictor, a distinct 512-cycle no-attempt stage records
    causal predictions and actual completion intervals for weight estimation.
    `WarmupSelfHits[d]` therefore has 512 observations and
    `score[d] = 512 / max(1, WarmupSelfHits[d])`. Identity, clock epoch, mode, or
    traffic-class replacement resets both stages. The two absorbed strata add
    about 19 seconds of warm-up per device at 60 Hz and do not change the rounded
    capacity budget.
18. **Current required AMD cohort.** The maintainer's current required board is
    Radeon RX 6800 XT (Navi 21/RDNA2, `amdgpu`). The maintainer reported the
    hardware change, abbreviated as “RX 6800”, in
    [PR #95](https://github.com/joske/yserver/pull/95#issuecomment-5393903848);
    the exact XT SKU was clarified for this C.0 revision. Every required AMD
    probe, final-tip matrix, and soak moves to this cohort.
19. **Historical Polaris scope.** RX 580/Polaris is no longer available to the
    maintainer and is currently unvalidated for C.0. Existing captures remain
    provenance only and cannot satisfy the current AMD matrix. The
    `c09358a1` VT scenario records its Polaris origin but must be replayed on the
    RX 6800 XT; any future Polaris run is best-effort supplementary evidence.

## Spike deadline, transport policy, and delivery-format follow-up

A later Opus review used a stale line snapshot for the already-resolved
64-versus-512 estimator and RX 580 replacement, but identified a real conflict
between the two-second returnability deadline and nvidia-drm's audited
three-second bounded waits. It also requested an explicit product policy for
the viable cursor transports and a single-PR delivery. The user approved:

20. **Cohort-audited return deadline.** Before the returnability arm, record the
    exact GPU, kernel, module version/`srcversion` and auditable source revision,
    then derive `SeatActiveReturnDeadline = max(2 seconds,
    KnownBoundedWaitMax + 2 seconds)`. The required NVIDIA cohort's three-second
    wait therefore yields five seconds. The review verified both `3 * HZ` sites
    in `nvidia-drm-modeset.c` against open-module source `610.57.04` at
    `e4a5faa2567f28c8eabe0ebb6422b6d0abcf37eb`; this is review provenance, not a
    substitute for recording the module actually used by the spike. Unknown
    source mapping, arithmetic failure, or stale driver/kernel evidence is
    `EvidenceInsufficient`, not process-isolation evidence. The executor's
    production watchdog remains a separate operational policy.
21. **Measured transport viability and state-derived preference.** No vendor
    family selects a coordinate transport. Exact-cohort evidence qualifies the
    viable set. A qualified owner-mediated legacy move wins in composed and
    direct state; otherwise composed state uses software cursor and direct state
    uses synchronous atomic motion only after its continuous-primary gates pass,
    falling back through the ordered software/unflip transition. Driver, kernel,
    plane, mode or relevant topology change invalidates qualification. Phase C.2
    may replace this preference table.
22. **NVIDIA synchronous-atomic spike arm.** The RTX 5060 Ti additionally runs
    `SynchronousAtomicMove` under continuous composed and direct primary load,
    recording slot occupancy, completion, `EBUSY`, host-call duration,
    coordinate rate and primary FPS even when software cursor remains selected.
23. **Named immediate policy.** C.0 names its admission timing
    `DispatchTimingPolicy::ImmediateOnRetirement`; it runs admission in the
    retirement wake without a retention timer. Phase C.2 owns any later change.
24. **Single-PR delivery.** C.0 is one PR to `master` and one confirmed squash
    merge. The former four PR families remain ordered implementation/review
    stages, with owner integration before cursor conversion, but are not merge
    or evidence boundaries. One consolidated physical campaign runs against the
    identical final integrated tip and the capability enters `master` only in
    the complete squash commit.
