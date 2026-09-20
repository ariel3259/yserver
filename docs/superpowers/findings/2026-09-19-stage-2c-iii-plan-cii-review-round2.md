## Verdict

**1 blocking, 1 major, 0 minor**

**Coverage: COMPLETE FOR DECLARED SCOPE**

This is a design-review result, not a claim that the code compiles, tests pass, or implementation is approved.

## Incorporation audit

| Prior finding | Status | Audit |
|---|---|---|
| B-1 — Missing `Presented` lacked a fallback-clock source | **TRADED** | Decision 6 adds a source and `(CommitId, reference CRTC)` binding, but its fallback rule contradicts its test and relies on an API that fabricates `(0,0)` when no sample exists ([plan lines 23–25](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-19-phase-c0-stage-2c-iii-plan-cii-direct.md:23), [lines 168–174](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-19-phase-c0-stage-2c-iii-plan-cii-direct.md:168)). See B-1. |
| M-1 — §5.0 evidence missed context validation and real dispatch failure | **APPLIED** | Task 1 now uses an independently invalid `CompletionContext` before ledger invocation and adds a real direct-dispatch registration-failure case ([plan lines 93–100](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-19-phase-c0-stage-2c-iii-plan-cii-direct.md:93)). Its mutation accounting remains incomplete; see M-1. |
| M-2 — Task 4 rested on the injected source | **APPLIED** | Task 4 now introduces the minimum production request/resource interfaces, and its three owner-route scenarios are explicitly rerun after Tasks 5–6 complete the production source ([plan lines 134–146](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-19-phase-c0-stage-2c-iii-plan-cii-direct.md:134), [lines 164–176](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-19-phase-c0-stage-2c-iii-plan-cii-direct.md:164)). The omitted Legacy rerun does not depend on that source. |
| M-3 — S4/S6 lacked killing scenarios | **APPLIED** | Task 2 now includes a candidate ineligible only through the CRTC gate; Task 3 requires a real layout mutation between `decide` and `lock` ([plan lines 110–114](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-19-phase-c0-stage-2c-iii-plan-cii-direct.md:110), [lines 124–128](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-19-phase-c0-stage-2c-iii-plan-cii-direct.md:124)). |
| M-4 — Newer cursor could be retired without detection | **APPLIED** | Task 7 adds the newer-generation ordering case and S24 ([plan lines 182–188](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-19-phase-c0-stage-2c-iii-plan-cii-direct.md:182)). |

## Findings

### Blocking

#### B-1 — Decision 6 defines mutually incompatible `Skip` clock semantics

The spec requires a missing-`Presented` request to emit `Skip` using the reference CRTC’s last validated clock sample ([spec lines 256–261](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-19-phase-c0-stage-2c-iii-conversion-design.md:256)). Decision 6 adopts that rule, but Task 6 says a `Skip` stamped from another commit’s `Presented` must fail ([plan line 25](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-19-phase-c0-stage-2c-iii-plan-cii-direct.md:25), [lines 168–174](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-19-phase-c0-stage-2c-iii-plan-cii-direct.md:168)). Those requirements conflict: if commit A supplied the last validated sample and commit B has no `Presented`, A’s sample is precisely the required fallback for B.

The actual interface also cannot implement the claimed no-sample branch: `present_get_completion_clock` returns a fabricated `(msc=0, ust=0, PageFlip)` rather than absence ([platform.rs lines 5241–5250](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/platform.rs:5241)). Moreover, routed `Presented` samples update that store before commit correlation is checked ([backend.rs lines 19898–19923](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/backend.rs:19898)), despite the spec requiring unknown `CommitId`s to be ignored ([spec lines 270–274](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-19-phase-c0-stage-2c-iii-conversion-design.md:270)).

Concrete failure: A records sample `SA`; B is accepted but produces no `Presented`; B retires. The spec requires `Skip(SA)`, while Task 6 rejects a sample from another commit. With no earlier sample, the named getter silently produces forbidden zeroes. An unknown/stale `Presented` can also overwrite the fallback before B retires.

Smallest correction: distinguish B’s commit-bound `Flip` sample from the per-CRTC historical sample allowed for `Skip`; require commit validation before `Presented` updates platform clocks; make fallback lookup fallible (`Option`/`Result`); and define executable production behavior guaranteeing or handling absence. “F8 stop” is an implementation-time escalation, not runtime failure semantics or a passing test oracle.

### Major

#### M-1 — Task 1’s new contracts are absent from the mutation exit table

Task 1’s real-dispatch test asserts token abort, unchanged admission state, exact resource return, and registration under the record’s `CommitId` ([plan lines 97–100](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-19-phase-c0-stage-2c-iii-plan-cii-direct.md:97)). It also requires `begin_with_ledger` to continue refusing Present-bearing descriptions ([plan line 95](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-19-phase-c0-stage-2c-iii-plan-cii-direct.md:95)).

The exit table assigns only S1/S2 to Task 1, covering context validation and owner-slot cleanup ([plan lines 67–70](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-19-phase-c0-stage-2c-iii-plan-cii-direct.md:67)). It omits the spec’s required mutations for `confirm` instead of `abort`, registration under a different `CommitId`, and removing the old entry’s Present refusal ([spec lines 586–590](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-19-phase-c0-stage-2c-iii-conversion-design.md:586)). The coordinator applies only S1–S24 ([plan line 198](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-19-phase-c0-stage-2c-iii-plan-cii-direct.md:198)), so vacuous or incomplete versions of the new test are never challenged.

Smallest correction: add exit rows and killing mutations for failure-path `confirm`, wrong registration `CommitId`, and allowing Present through `begin_with_ledger`, assigning each to Task 1’s named evidence.

### Minor

None.

## Coverage and implementation checks

- **Incorporation:** all five round-1 findings checked against the revised task text.
- **Architecture/contracts:** checked production-source ownership, Task 4’s scope move/rerun, owner-event routing, direct confirmation, and clock identity flow.
- **Safety/ownership:** checked registration rollback, admission abort/confirm boundary, frame/commit correlation, retirement ordering, and fallback-clock provenance.
- **Spec/verification:** checked §§3.1–3.4, 5.0–5.7, and 8.1–8.3, plus every S1–S24 mapping.

Used **24/24 bounded excerpts**. Not assessed—and not asserted sound—are the eventual layout-site enumeration, detailed resource-service internals, fixture implementation, and cursor-state internals. Exact Rust signatures, compilation, formatting, clippy, portability, GPU execution, and actual mutation execution remain deferred to implementation.