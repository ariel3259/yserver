## Verdict

**1 blocking, 4 major, 0 minor**

**Coverage: COMPLETE FOR DECLARED SCOPE**

This is a bounded design-review result, not a claim that the plan compiles, passes tests, or is approved for implementation.

## Incorporation audit

| Prior finding | Status |
|---|---|
| None | First review; check 1 skipped as directed. |

## Findings

### Blocking

#### B-1 — Missing `Presented` has no defined source for the required fallback clock sample

The plan requires an accepted Present without `Presented` to publish `Skip` using “the last validated clock sample” ([plan lines 152–160](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-19-phase-c0-stage-2c-iii-plan-cii-direct.md:152)), matching the normative requirement ([spec lines 251–261](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-19-phase-c0-stage-2c-iii-conversion-design.md:251)). But it defines no carrier for that sample.

`OwnerEvent::Presented` is the only stated event containing samples ([device.rs lines 40–54](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/owner/device.rs:40)). `CompletionContext` supplies clock identities and validates the owner’s internal reference, but carries no `ClockSample` back to the frame ([device.rs lines 1286–1352](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/owner/device.rs:1286)). The frame has an optional legacy completion clock ([backend.rs lines 588–603](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/backend.rs:588)), but the plan never states that it is initialized from an authoritative validated sample for the owner route.

Concrete failure: commit A is accepted, no valid page event produces `Presented`, then `CompletionRetired` arrives. Retirement must publish a `Skip`, but the frame has received no sample. An implementation must either fabricate/reuse an unrelated timestamp, omit terminalization, or invent an unstated clock lookup—each violating the contract.

Required correction: define the authoritative source and handoff of the last validated sample for the frame’s reference CRTC, bind it with `(CommitId, reference CRTC)`, and test missing `Presented` with deliberately distinct clock samples so an unrelated CRTC or stale commit cannot satisfy it.

### Major

#### M-1 — The §5.0 tests do not exercise completion-context validation or direct-route ledger failure semantics

Mutation S1 removes completion-context validation, but the described test only supplies a consumer outside the kernel event set ([plan lines 86–90](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-19-phase-c0-stage-2c-iii-plan-cii-direct.md:86)). That rejection comes from closure construction—`PresentConsumerOutsideEventSet` ([closure.rs lines 214–223](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/owner/closure.rs:214))—and therefore still occurs if `validate_completion_context` is deleted. S1 can survive its named test.

The plan also lacks a real-direct-dispatch case where registration fails after `lock`: the spec requires `abort`, no consumed admission state, and obligations registered under the record’s own `CommitId` ([spec lines 583–590](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-19-phase-c0-stage-2c-iii-conversion-design.md:583)). The owner-only “leaves nothing” test cannot establish the conductor outcome.

Required correction: add (1) a structurally valid description with an independently invalid `CompletionContext`, proving refusal before the ledger closure, and (2) a real direct-dispatch registration failure proving owner cleanup, token abort, unchanged fairness state, and exact resource return.

#### M-2 — Task 4’s required evidence either rests on the injected source or depends on interfaces introduced by Tasks 5–6

The baseline explicitly says the injected `AdmissionSource` remains authoritative for direct answers ([admission.rs lines 237–270](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/admission.rs:237)), and snapshotting calls its `direct_eligible` method ([admission.rs lines 895–913](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/admission.rs:895)). Yet Task 4 requires retirement admission and owner dispatch tests before Tasks 5 and 6 introduce production members, leases, and the direct description builder ([plan lines 122–160](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-19-phase-c0-stage-2c-iii-plan-cii-direct.md:122)).

Thus Task 4 can run in order only by dispatching a synthetic source description, while the spec forbids any exit criterion resting on the injected source ([spec lines 132–141](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-19-phase-c0-stage-2c-iii-conversion-design.md:132), [lines 554–559](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-19-phase-c0-stage-2c-iii-conversion-design.md:554)). A broken or unreachable production description builder could therefore coexist with passing promotion-order evidence.

Required correction: move the minimum production direct request/resource interfaces before Task 4, or explicitly require the Task 4 tests to be converted and rerun through the final production-backed source after Tasks 5–6.

#### M-3 — Eligibility mutations S4 and S6 are not killed by the stated scenarios

S4 removes `direct_present_crtc_eligible`, but the deterministic table compares only against `scanout_direct_eligible`, whose inputs do not include that gate, while the route comparison uses one unspecified candidate ([plan lines 96–102](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-19-phase-c0-stage-2c-iii-plan-cii-direct.md:96)). If that candidate is CRTC-eligible, S4 survives.

S6 removes the stale-generation check at `lock`. Ordinary border-hook tests can invalidate the queued successor before decision/lock, never exercising the lock boundary required by 2c-ii ([2c-ii spec lines 281–297](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-18-phase-c0-stage-2c-ii-admission-design.md:281)).

Required correction: include an explicitly CRTC-ineligible candidate for S4, and a controlled real layout mutation between `decide` and `lock` for S6, asserting the owner receives nothing.

#### M-4 — The newer-cursor non-retirement invariant has neither dedicated evidence nor a killing mutation

Section 5.6 requires both that direct commits omit unchanged cursor generations and that a primary flip cannot retire a newer cursor generation ([spec lines 465–468](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-19-phase-c0-stage-2c-iii-conversion-design.md:465)). The plan assigns one test and S22 solely to “carry the unchanged cursor generation” ([plan lines 78 and 166–172](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-19-phase-c0-stage-2c-iii-plan-cii-direct.md:166)).

A handler that incorrectly marks generation N+1 retired when commit N completes can pass the named test. Add a withheld/newer-generation ordering case and a mutation that retires the live newer generation.

## Coverage and implementation checks

- **Incorporation:** skipped; no prior review.
- **Architecture/contracts:** checked task ordering, real-versus-injected admission source, retirement wake ordering, one-fork feasibility, member identity, and Present correlation.
- **Safety/ownership:** checked two-phase confirmation, pin/frame ownership at the stated contract level, missing-sample terminalization, commit identity, and deferred publication ordering.
- **Spec/verification:** checked §§3.2, 3.3, 5.0–5.7 and applicable §8.2 rows, including named mutations.

Used **24/24 bounded excerpts**. Verified ground includes the owner entry structure, closure consumer validation, admission snapshot, legacy eligibility inputs, direct-frame state, managed retirement enqueue, and owner-event batch ordering.

Not assessed—and not asserted sound—are every concrete layout mutation site, the full direct ledger closure, detailed resource-service internals, and fixture implementations. Exact Rust signatures, borrow/trait behavior, compilation, formatting, clippy, portability builds, GPU execution, and mutation execution remain deferred to implementation.