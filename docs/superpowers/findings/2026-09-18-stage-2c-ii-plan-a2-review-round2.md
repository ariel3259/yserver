## Verdict

**2 blocking, 1 major, 0 minor**

Coverage: **COMPLETE FOR DECLARED SCOPE**

**Target:** plan A2 revision 2 (`84366678`), with prior round 1.

**Reviewer:** `codex exec --sandbox read-only`, single pass
**Instrument:** `docs/superpowers/review/` @ `69c6d6e2`;
model `gpt-5.6-sol`; reasoning effort `xhigh`; `codex-cli 0.154.0`.
Counts are comparable only to other reviews citing this same instrument SHA.

**Recorded usage:** 75,509 tokens (exit 0). 12/12 excerpts.

**Author verification (2026-09-18):**

- **B-1 — CONFIRMED.** `defer_direct_successor_skip` (`backend.rs:2552`) pushes to `deferred_successor_skips`, never `completed`; a Skip created during the retirement wake, after the only append, has no later retirement to publish it. Also true with nothing in flight at all. Fixed by a global rule: at the end of every conductor entry point, with no predecessor in flight, append the deferred skips.
- **B-2 — CONFIRMED, and wider.** The existing seam's `reserve(OrdinaryRetirement)?` (`backend.rs:19844`) runs after `attach` and drops the attached resources on error (a latent 2c-i defect, hard to reach behind the earlier `is_vacant` check). Fixed: transactional prepare (reserve first), and every post-lock exit aborts the token.
- **M-1 — CONFIRMED.** Fixed: `take_current` and `composed_resources` inside the builder; the owner test covers every refusal point of `begin_with_context` in order (identity exhaustion reachable via `next_seq`, as the existing `sequence_exhaustion_refuses_without_reserving` does).

This is a bounded design-review result, not a claim that the plan compiles, tests pass, or is approved for implementation.

## Incorporation audit

| Prior finding | Status | Assessment |
|---|---|---|
| B-1 — post-`begin` bind cannot populate the ledger | **PARTIAL** | `begin_with_ledger` fixes the principal ownership boundary by reserving the slot before invoking the ledger builder ([plan:95](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-18-phase-c0-stage-2c-ii-plan-a2-conductor.md:95)). Direct undo is specified and tested. However, composed resource acquisition and the intermediate refusal cases remain under-specified and under-tested; see M-1. |
| B-2 — descriptor/frame transaction breaks across unflip | **APPLIED** | Offers are rejected before touching the seam while an unflip is pending, and unflip removes the matching frame and reservation through the never-submitted path ([plan:208](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-18-phase-c0-stage-2c-ii-plan-a2-conductor.md:208)). This matches the existing discharge/defer behavior ([backend.rs:19882](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/backend.rs:19882)). |
| M-1 — missing live direct eligibility | **APPLIED** | `AdmissionSource::direct_eligible` is now a snapshot input; false withdraws and terminalizes the successor, with a named mutation and test ([plan:169](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-18-phase-c0-stage-2c-ii-plan-a2-conductor.md:169), [plan:213](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-18-phase-c0-stage-2c-ii-plan-a2-conductor.md:213)). |
| M-2 — retirement test cannot prove ordering | **APPLIED** | The exact `Consumed → Enqueued → Decided → Dispatched` operation trace and a separate no-early-drain assertion are required ([plan:293](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-18-phase-c0-stage-2c-ii-plan-a2-conductor.md:293)). |
| M-3 — no non-empty-current refusal evidence | **APPLIED** | The plan now requires both direct and composed refusals over non-empty current state and asserts restoration of `ResourcesStillCurrent` plus disposition of released-new resources ([plan:275](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-18-phase-c0-stage-2c-ii-plan-a2-conductor.md:275)). |

## Findings

### Blocking

#### B-1 — Retirement-time invalidation or refusal strands the successor’s `Skip`

Task 4 appends the predecessor and all currently deferred skips to `completed`, then calls `admission_wake` ([plan:293](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-18-phase-c0-stage-2c-ii-plan-a2-conductor.md:293)). During that wake, either eligibility invalidation or a pre-IPC direct refusal calls the never-submitted path ([plan:213](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-18-phase-c0-stage-2c-ii-plan-a2-conductor.md:213), [plan:267](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-18-phase-c0-stage-2c-ii-plan-a2-conductor.md:267)). The real helper places the new `Skip` back into `deferred_successor_skips`, not `completed` ([backend.rs:2552](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/backend.rs:2552)).

Concrete sequence: A retires; its completion and old skips are enqueued; snapshot discovers successor B is now ineligible—or B reaches `send_on` and gets `Reaped`; B is idled and its `Skip` is deferred after the only append point; the handler returns and publishes A. If no later retirement occurs, B’s `Skip` is never published. This violates invalidation’s required predecessor-ordered terminalization ([spec:170](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-18-phase-c0-stage-2c-ii-admission-design.md:170)) and the conductor’s enqueue/admit/publish contract ([spec:303](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-18-phase-c0-stage-2c-ii-admission-design.md:303)).

Smallest correction: after the retirement wake, append skips newly produced by that wake to `completed` before returning, preserving A before B. Add retirement-wake cases for both eligibility invalidation and pre-IPC refusal.

#### B-2 — Post-lock preparation failures have no token or resource disposition

The conductor locks the decision before building the request ([plan:240](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-18-phase-c0-stage-2c-ii-plan-a2-conductor.md:240)). Direct preparation nevertheless returns `Result<Option<PreparedDirectDispatch>, ResourceError>` ([plan:121](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-18-phase-c0-stage-2c-ii-plan-a2-conductor.md:121)). Task 3 specifies abort for `begin`, `send_on`, and lock mismatch, but gives neither `Err` nor `Ok(None)` after `lock` an outcome or cleanup contract.

These are real failure boundaries: the existing seam can fail while moving the successor role and while reserving retirement capacity ([backend.rs:19814](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/backend.rs:19814), [backend.rs:19844](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/backend.rs:19844)).

Concrete sequence: snapshot admits a queued successor; `lock` creates the pending token; preparation encounters closed/mismatched capacity and returns `Err`. No specified branch aborts the token or defines whether partially moved resources are restored. The decider may remain permanently locked, contrary to the requirement that every token be consumed exactly once ([spec:277](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-18-phase-c0-stage-2c-ii-admission-design.md:277)).

Smallest correction: define a preparation-refused outcome; require `abort(token)` on every post-lock/pre-`begin` exit; make preparation’s `Err` ownership transactional or explicitly fail closed; test both `Err` and `Ok(None)` with a mutation that omits the abort.

### Major

#### M-1 — The ledger-builder correction is not proven across all refusal points or composed resources

The new contract correctly says the builder runs only after every refusal point ([plan:95](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-18-phase-c0-stage-2c-ii-plan-a2-conductor.md:95)), but its test covers only preliminary `page_flip_event` rejection and final slot occupancy ([plan:152](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-18-phase-c0-stage-2c-ii-plan-a2-conductor.md:152)). Actual intermediate failures include three identity checks ([device.rs:1224](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/owner/device.rs:1224)), request construction, and context validation before reservation ([device.rs:1328](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/owner/device.rs:1328)).

Additionally, the composed path does not explicitly require `composed_resources` to be called inside the delayed builder; it only says refused `begin` leaves current state untouched ([plan:260](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-18-phase-c0-stage-2c-ii-plan-a2-conductor.md:260)). An implementation can pre-fetch non-empty composed resources, capture them in the closure, and drop the returned closure after an intermediate refusal. Existing named tests still pass while leases or reservations are lost.

Smallest correction: require both `composed_resources` and `take_current` to execute inside the builder, and test non-empty resource ownership across legacy, identity/build/context, and slot refusals—or use an ordering probe that establishes the builder follows all of them.

### Minor

None.

## Coverage and implementation checks

- Incorporation: audited all five prior findings; one remains partial.
- Architecture/contracts: traced owner reservation, conductor locking, direct preparation, retirement routing, and event publication.
- Safety/ownership: traced role discharge, never-submitted deferral, ledger construction, abort paths, and post-retirement ordering.
- Spec/verification: checked relevant §§4, 6, 7, and 10.2, including named mutations and fixture limits.

Excerpts used: **12/12**. Verified ground includes actual `next_correlation`/`begin`, direct reservation and unflip behavior, `defer_direct_successor_skip`, `route_owner_event`, and the `CommitResourceConsumer` entry. The unshown remainder of `consume`, the omitted middle of the existing direct-dispatch seam, and A1 token internals were not independently audited and are not claimed sound.

Compilation, Rust borrow/signature details, fmt, clippy, tests, mutation runs, target portability checks, and stated hardware limits remain deferred to implementation.