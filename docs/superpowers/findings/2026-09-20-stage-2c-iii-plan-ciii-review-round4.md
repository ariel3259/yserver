## Verdict

**1 blocking, 1 major, 0 minor**

Coverage: COMPLETE FOR DECLARED SCOPE

This is a design-review result only; it does not claim compilation, passing tests, or implementation approval.

## Incorporation audit

| Prior finding | Status | Result |
|---|---|---|
| R3 B-1 — recursive Owner request path | **APPLIED** | Decision 3 makes the funnel→admission direction one-way, removes the reverse callback, and names both affected test callers ([plan line 158](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-20-phase-c0-stage-2c-iii-plan-ciii-unflip.md:158)). |
| R3 B-2 — equal numeric commit IDs cross devices | **APPLIED** | Task 6 requires device-qualified resource identities/maps and the exact interleaving from the finding ([plan line 550](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-20-phase-c0-stage-2c-iii-plan-ciii-unflip.md:550)). M-1 identifies additional non-resource correlations beyond round 3’s demonstrated sequence. |
| R3 M-1 — failed dispatch leaves the wrong capacity role | **APPLIED** | Task 2 requires restoration to `Current`, cancellation of the exit reservation, an explicit assertion, and T35 ([plan line 407](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-20-phase-c0-stage-2c-iii-plan-ciii-unflip.md:407)). |

## Findings

### Blocking

#### B-1 — The promised materialization retry has no event-loop edge

The plan assigns preparation and retry to the `request_direct_unflip` fork, asserting that a failure is retried “on each tick” ([plan line 153](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-20-phase-c0-stage-2c-iii-plan-ciii-unflip.md:153), [plan line 361](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-20-phase-c0-stage-2c-iii-plan-ciii-unflip.md:361)). But that funnel is cause-driven: it merely records flags when called ([backend.rs line 2315](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/backend.rs:2315)). The tick currently calls it only while cursor/overlay conditions remain true; an already-requested unflip otherwise advances to the transaction branch without re-entering preparation ([backend.rs line 21564](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/backend.rs:21564)).

Concrete sequence: the spec’s one-shot failed-successor-send cause requests an Owner unflip; shadow materialization fails; the intent remains requested and unready; the cause disappears. Later ticks never re-enter the request fork, so the shadow cannot become ready and the unflip stalls permanently. This violates the required cause routing and materialized-shadow readiness ([spec lines 481–490](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-19-phase-c0-stage-2c-iii-conversion-design.md:481)). T6/T7 address false readiness and side effects, not absence of the retry scheduler.

Smallest correction: name an unconditional real-tick edge that, while an Owner unflip is requested and its shadow is unmaterialized, retries preparation before waking admission without repeating terminalization. Require the failure test to raise a one-shot cause once, fail one attempt, then drive an ordinary later tick without re-raising that cause.

### Major

#### M-1 — Device-qualified resource maps leave other bare-CommitId consumers unverified

Task 6 says “every commit correlation” becomes device-qualified, but its concrete inventory, equal-ID test, and T36/T37 cover resource caches, retirements, rejected resources, and obligation matching only ([plan line 554](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-20-phase-c0-stage-2c-iii-plan-ciii-unflip.md:554), [plan line 583](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-20-phase-c0-stage-2c-iii-plan-ciii-unflip.md:583)).

Existing non-resource consumers still correlate globally by numeric commit:

- `PresentKey` contains a device, but terminal/presented handling compares only `key.commit` ([present.rs line 3](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/resources/present.rs:3), [commit.rs line 394](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/resources/commit.rs:394)).
- Event routing has `device_key` but drops it when calling the shared consumer ([backend.rs line 20361](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/backend.rs:20361)).
- The pending direct frame records only `CommitId`, and `CompletionUnknown` matches it without a device before terminalizing the Present and releasing both logical pins ([backend.rs line 588](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/backend.rs:588), [backend.rs line 2919](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/backend.rs:2919)).

Concrete sequence: device A has pending direct commit 1; device B’s distinct commit 1 reaches `FailedBeforeSubmit` or `CompletionUnknown`. B’s terminal event can suppress A’s disposition or mark A’s Present skipped and release A’s pins without evidence from A. That violates the isolation requirement that an event on one device change nothing on another ([spec lines 496–500](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-19-phase-c0-stage-2c-iii-conversion-design.md:496)).

Smallest correction: explicitly include Present dispositions, terminal/presented consumers, and pending direct-frame identity in Task 6’s key conversion. Extend the equal-ID test with a B terminal event while A’s direct Present is pending, plus a mutation removing the device comparison.

### Minor

None.

## Coverage and implementation checks

All four checks were performed using **24/24 bounded spec/source excerpts**. Verified ground included round-3 incorporation, request/readiness/tick delivery, dispatch rollback, unflip return contracts, resource and Present correlation, device event routing, hardware evidence, mutations, and portability gates.

Unassessed and not declared sound: exhaustive unflip-cause call sites, exhaustive device-blind helpers, fixture construction, and real-card retained-allocation reachability. The excerpt budget is exhausted.

Compilation, formatting, clippy, cross-target checks, Vulkan/hardware execution, and mutation runs remain deferred to implementation.