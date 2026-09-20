# Stage 2c-iii plan Ci-refactor — codex review, round 2

**Target:** plan Ci-refactor revision 3 (`97ac6e99`), with round 1 as the prior
review. Tasks 1-3 were already implemented and committed at that point.

**Reviewer:** `codex exec --sandbox read-only`, single pass
**Instrument:** `docs/superpowers/review/` @ `da807b70`;
model `gpt-5.6-sol`; reasoning effort `xhigh`.
Counts are comparable only to other reviews citing this same instrument SHA.

**Author verification (2026-09-19), both findings checked against the tree:**

- **B-1 — CONFIRMED.** Spec §8.3 defined this plan as "Behaviour-preserving, no
  new design", accepted only if every result and the hardware gate are
  unchanged, so no Task 4 — revision 2's or revision 3's — had authority for a
  policy change. Fixed by amending §8.3 first (`a1adc3b1`, the user's decision),
  narrowly: reachable rows keep today's behaviour, the unreachable ones are
  identified by recorded measurement, and the acceptance numbers, the mutation
  parity and the hardware gate stay unchanged. Plan revision 4 records it.
- **M-1 — CONFIRMED.** The matrix test drives only reachable rows, so mutating
  the collapsed fallback could not fail it. Fixed: Task 4 adds
  `c0_conv_cir_dispatch_failure_fallback_is_fail_closed`, a policy-level test on
  the real policy function, and owns both mutations itself.

**Two findings the coordinator raised on Task 4's implementation, beyond this
review:** F-T4R-1, the reachable direct `Refused` row had been split by the undo
result; and F-T4R-2, the claim that an undo failure is unreachable missed
`DirectCapacity::reserve`'s first refusal (`capacity.rs:136`), an already-closed
admission. The row now ignores the undo result, as Ci did, and the matrix test
covers the closed-admission case.

---

## Verdict

**1 blocking, 1 major, 0 minor**

**Coverage: COMPLETE FOR DECLARED SCOPE**

This is a design-review result only; it does not claim that the code compiles, tests pass, or implementation is approved.

## Incorporation audit

| Prior finding | Status | Result |
|---|---|---|
| B-1 — shared cleanup policy changes behavior | **TRADED** | Tasks 1–3 correctly preserved and measured the existing matrix ([plan:32](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-19-phase-c0-stage-2c-iii-plan-ci-refactor.md:32)). Task 4 now deliberately replaces the unreachable cells, but does so without amending the authoritative behavior-preserving scope; see B-1 below. |
| B-2 — quarantine retains discarded payload | **APPLIED** | `Quarantined` is explicitly identity-only and drops the pending acknowledgement, lease, descriptor slot, and commit identity ([plan:29](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-19-phase-c0-stage-2c-iii-plan-ci-refactor.md:29), [plan:80](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-19-phase-c0-stage-2c-iii-plan-ci-refactor.md:80)). Task 4 does not contradict it. |
| M-1 — missing identity and ordering contract | **APPLIED** | Immutable identity, generation ordering, original-position restoration, and single-current cardinality are specified ([plan:28](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-19-phase-c0-stage-2c-iii-plan-ci-refactor.md:28), [plan:31](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-19-phase-c0-stage-2c-iii-plan-ci-refactor.md:31)). |
| M-2 — binary membership test cannot replace logical-state guard | **APPLIED** | The plan adds exact lifecycle-state assertions and explicitly reapplies R19 against the new release helper ([plan:92](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-19-phase-c0-stage-2c-iii-plan-ci-refactor.md:92), [plan:138](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-19-phase-c0-stage-2c-iii-plan-ci-refactor.md:138)). |

## Findings

### Blocking

#### B-1 — Task 4 introduces a policy change forbidden by the authoritative spec

The authoritative specification defines the entire Ci-refactor as “Behaviour-preserving, no new design” and accepts it only if test and hardware behavior remain unchanged ([spec:612](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-19-phase-c0-stage-2c-iii-conversion-design.md:612)). It also states that 2c-iii feeds the existing conductor and “does not change an admission rule” ([spec:51](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-19-phase-c0-stage-2c-iii-conversion-design.md:51)).

Task 4 is expressly labeled the only behavior change and replaces existing cleanup/missing-resource policies with a new universal fail-closed rule ([plan:20](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-19-phase-c0-stage-2c-iii-plan-ci-refactor.md:20), [plan:114](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-19-phase-c0-stage-2c-iii-plan-ci-refactor.md:114), [plan:122](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-19-phase-c0-stage-2c-iii-plan-ci-refactor.md:122)). The current policy demonstrably assigns different gate states and outcomes to those cells ([admission.rs:127](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/admission.rs:127)).

Concrete failure scenario: Task 4 is implemented exactly as written; all production-driven tests remain green because the changed cells are unreachable, while the plan and source now establish an admission policy that the higher-authority specification explicitly forbids this refactor from designing. The lack of a runtime witness does not let a subordinate plan revise its authority.

Smallest correction: amend authoritative §8.3 first to authorize this narrowly scoped policy for production-unreachable invariant failures, or remove Task 4 and retain the measured policy.

### Major

#### M-1 — The reachable-only matrix cannot catch mutation of the unreachable fallback

Task 4 requires the matrix test to contain only production-reachable cases plus a textual note, yet also requires it to fail when the fail-closed fallback is changed to leave the gate open and return `BeginRefused` ([plan:126](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-19-phase-c0-stage-2c-iii-plan-ci-refactor.md:126)).

The existing test drives the four observable conditions and explicitly records that all fallback conditions cannot be produced through the production closures ([backend.rs:56050](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/backend.rs:56050), [backend.rs:56240](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/backend.rs:56240)). Therefore, changing only the catch-all action cannot affect any proposed test case. The required mutation survives, so the evidence cannot establish the newly introduced policy.

Smallest correction: add a policy-level test against the real production policy function for a representative collapsed condition, asserting closed gate/`TransportClosed`, and state explicitly that this proves only the fallback mapping—not production reachability or end-to-end resource restoration. Assign the stated mutation check to Task 4’s execution/acceptance steps.

### Minor

None.

## Coverage and implementation checks

- **Incorporation:** audited all four prior findings against revision 3.
- **Architecture/contracts:** inspected the shared failure handler, both dispatch producers, restoration paths, and the real matrix cases. No reachable behavior lost by Task 4 was found.
- **Safety/failure semantics:** verified that the shared handler aborts the token unconditionally ([admission.rs:1160](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/admission.rs:1160)); this agrees with 2c-ii’s abort-without-fairness-consumption and no-immediate-retry rules ([2c-ii spec:290](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-18-phase-c0-stage-2c-ii-admission-design.md:290), [2c-ii spec:326](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-18-phase-c0-stage-2c-ii-admission-design.md:326)). Closing admission does not conflict with required drainage of already-dispatched records ([2c-ii spec:387](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-18-phase-c0-stage-2c-ii-admission-design.md:387)).
- **Spec/verification:** found the authority conflict and the unwitnessed fallback mutation above. Build, formatting, clippy, test, release, and hardware gates remain assigned to implementation.

**Excerpts used: 23/24**, excluding the complete plan and prior-review reads. Per the declared boundary, the accepted reachability measurement itself was not re-audited; `device.rs`/`slot.rs` were not inspected because no missed production path emerged. Tasks 1–3, exact Rust shape, logging implementation, portability, compilation, and actual test execution remain unassessed or deferred to the real compiler/tests.