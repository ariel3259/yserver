# Stage 2c-iii plan Ci-refactor — codex review, round 1

**Target:** `docs/superpowers/plans/2026-09-19-phase-c0-stage-2c-iii-plan-ci-refactor.md`
revision 1 (`bdc44848`).

**Reviewer:** `codex exec --sandbox read-only`, single pass
**Instrument:** `docs/superpowers/review/` @ `da807b70`;
model `gpt-5.6-sol`; reasoning effort `xhigh`; `codex-cli 0.155.1`.
Counts are comparable only to other reviews citing this same instrument SHA.

**Author verification (2026-09-19), every finding checked against the tree:**

- **B-1 — CONFIRMED, and wider.** The primary and direct failure arms differ in
  more than the reviewer's cell (`admission.rs` ~1152–1180 and ~1320–1390):
  primary Cleanup closes the gate when the restore succeeds and not when it
  fails, and returns `BeginRefused` where direct returns `TransportClosed`;
  direct Ledger with no resource returned does not close the gate. Revision 2
  records the whole measured matrix, preserves it in Tasks 1–3, and fixes it in
  a separate Task 4 as an explicit behaviour change (our own code, user rule).
- **B-2 — CONFIRMED.** `CompletionUnknown` drops the scene record after
  quarantining the buffer ("no quarantine queue", `scene.rs` ~2124). Fixed:
  `Quarantined` keeps identity only and drops the payload.
- **M-1 — CONFIRMED.** Accepted: immutable identity and generation order.
- **M-2 — CONFIRMED.** R19's equivalence rested on the phase guard the refactor
  removes. Accepted: re-apply R19; add an exact-state sequence test.

Revision 2 incorporates all four.

---

## Verdict

**2 blocking, 2 major, 0 minor**

**Coverage: COMPLETE FOR DECLARED SCOPE**

This is a design-review result only; it does not claim that the plan compiles, passes tests, or is approved for implementation.

## Incorporation audit

| Prior finding | Status | Result |
|---|---|---|
| — | SKIPPED | No prior review exists, as stated in the brief. |

## Findings

### Blocking

#### B-1 — The shared cleanup policy changes primary-dispatch behavior

The plan requires the shared path to close the transport gate on cleanup failure ([plan:23](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-19-phase-c0-stage-2c-iii-plan-ci-refactor.md:23)), while also promising identical transport closure and outcomes ([plan:77](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-19-phase-c0-stage-2c-iii-plan-ci-refactor.md:77)). Those requirements contradict the accepted implementation.

Concrete sequence: primary `begin` returns `FallibleBeginError::Cleanup`, then restoring the composed resources fails. The current primary path aborts and returns `TransportClosed` without calling `gate.force_close()` ([admission.rs:1165](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/admission.rs:1165)). The direct cleanup path does close the gate ([admission.rs:1343](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/admission.rs:1343)). A helper implementing decision 4 literally therefore closes a previously open primary gate, changing subsequent dispatch behavior. That violates the spec’s behaviour-preserving requirement ([spec:612](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-19-phase-c0-stage-2c-iii-conversion-design.md:612)).

Smallest correction: add an explicit failure-policy matrix covering producer × error kind × restore success, including immediate outcome and gate state. Parameterize the shared mechanism by that policy. If closing the primary gate is intended as a fix, it is an F8 stop because it is outside this refactor.

#### B-2 — `Quarantined` retains resources that the accepted route deliberately drops

Decision 1 says `Quarantined` remains an `OwnerBuffer` state, carries the captured `PendingAck`, and—being “Submitted and later”—carries the `CommitId` ([plan:20](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-19-phase-c0-stage-2c-iii-plan-ci-refactor.md:20)). It has no exit.

The current `CompletionUnknown` path does the opposite: after physically quarantining the BO, it clears the commit identity and drops the scene record; the comment explicitly says there is no quarantine queue ([scene.rs:2124](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/scene.rs:2124)). `PendingAck` owns nontrivial payload including a fence ticket, damage snapshots, and cursor retirement data ([scene.rs:182](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/scene.rs:182)). The proposed terminal value would retain these indefinitely rather than releasing them, changing resource lifetime despite the BO correctly remaining unreusable. The spec permits quarantine but does not authorize retaining transaction payload; the refactor must preserve behavior ([spec:612-620](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-19-phase-c0-stage-2c-iii-conversion-design.md:612)).

Smallest correction: define `Quarantined` as a minimal poison marker containing only stable buffer identity. Its transition must explicitly discard `PendingAck`, descriptor data, lease remnants, and `CommitId`, with a test proving those payloads are no longer owned.

### Major

#### M-1 — The replacement collection lacks the identity and ordering contract Task 2 needs

The new collection is only specified as keyed by buffer index, with ordering “derived from the states” ([plan:22](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-19-phase-c0-stage-2c-iii-plan-ci-refactor.md:22)). States cannot order two `Submitted`, `Displaced`, or `Releasing` entries. Nor is buffer index the stable member identity required by the spec: transactions identify `(output, buffer index, generation)` ([spec:314](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-19-phase-c0-stage-2c-iii-conversion-design.md:314)), and topology-safe ownership requires member-specific old state ([spec:394](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-19-phase-c0-stage-2c-iii-conversion-design.md:394)).

Today `PreparedComposed` separately retains `OutputKey`, managed-allocation identity, generation, and commit identity ([scene.rs:1003](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/scene.rs:1003)); failed resolution restores a submitted member at its original queue index ([scene.rs:2047](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/scene.rs:2047)); current retirement additionally matches `OutputKey` ([scene.rs:2171](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/scene.rs:2171)).

Concrete failure: two same-state members cannot recover FIFO position or identify the correct pre-topology current member from state plus buffer index alone. Task 2 would have to invent an interface Task 1 did not provide.

Smallest correction: Task 1 must define immutable identity fields—at least `OutputKey`, CRTC, buffer index, generation, and managed allocation key—and an explicit total-order/cardinality contract, such as generation order with original-position restoration. Ordering must be derived from that key, not from enum state.

#### M-2 — Mutation parity relies on a guard the refactor deletes, and the new agreement test is binary only

The new test proves only:

`BoPhase == owner-held` iff an `OwnerBuffer` entry exists

([plan:67](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-19-phase-c0-stage-2c-iii-plan-ci-refactor.md:67)). Once all logical states share one physical phase ([plan:21](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-19-phase-c0-stage-2c-iii-plan-ci-refactor.md:21)), changing `Desired` to `Accepted`, leaving a buffer `Submitted`, or freeing through the wrong logical state does not violate that equivalence.

The parity list nevertheless says R19 keeps its old “equivalent” status ([plan:91](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-19-phase-c0-stage-2c-iii-plan-ci-refactor.md:91)). Its recorded equivalence specifically depended on `BoState::transition_to_free_after_owner_release` rejecting every phase except `OwnerReleasing` ([accepted finding:56](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/findings/2026-09-19-stage-2c-iii-plan-ci-accepted.md:56))—the exact phase guard this refactor removes.

Smallest correction: rename the new check as a membership-agreement test; add route-level assertions for the exact logical state and identity after every lifecycle milestone; and reapply R19 against the new release helper by mutating it to accept a non-`Releasing` `OwnerBuffer`. Do not inherit its old status.

### Minor

None.

## Coverage and implementation checks

- **Incorporation:** skipped correctly; no prior review.
- **Architecture/contracts:** checked collection ownership, stable identity, queue ordering, task dependencies, event resolution, and shared dispatch policy.
- **Safety/failure semantics:** checked lease/payload lifetime, quarantine, refusal/cleanup restoration, token abort, gate closure, commit correlation, and release ordering.
- **Spec/verification:** checked relevant spec §§3.2, 4.1–4.6, and 8.3; Ci decisions 10–11; accepted mutations/F8 stops; and the plan’s build, clippy, test, GPU, and hardware assignments.

**Excerpts used: 24/24**, excluding the one complete target-plan read and skill instructions. Exact owner-phase references were located only in `scanout.rs`, `platform.rs`, `scene.rs`, and backend tests. Not exhaustively inspected: generic non-`Free` phase consumers, individual mutation-test bodies, and the owner `begin` implementations; compiler-level signatures, borrow behavior, formatting, clippy, builds, portability, and test execution remain deferred to implementation.