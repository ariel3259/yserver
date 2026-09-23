# Stage 3a-i plan — codex review, round 1

**Target:** `docs/superpowers/plans/2026-09-23-phase-c0-stage-3a-i-plan-pure-arbiter.md`
revision 1 (`dd25d47e`). **Result:** 2 blocking, 4 major, 1 minor; coverage
COMPLETE FOR DECLARED SCOPE (11/24).

**Reviewer:** `codex exec --sandbox read-only`, single pass; instrument
`docs/superpowers/review/` @ `0245f96b`; `gpt-6-sol` `xhigh`; `codex-cli 0.155.1`.

**Author verification (2026-09-23):** all seven CONFIRMED against the plan text
and C.0 — B-1 (C.0 line 801 and item 57: supersession obligations must complete
before the winner opens or publishes; revision 1 had no acknowledgment for
them); B-2 (C.0 line 768: exactly one `RecoveryId` per incident; nothing
created the first); M-1 (Task 3 needed Task 4's matrix); M-2 (item 63 needs an
exact ledger under mixed arrivals); M-3 (item 65: ordinary work carries no
transition id); M-4 (a logical DPMS change while `Poisoned` retires no
projection); m-1 (C.0 line 867 latest-wins applies to a deferred
representative). Fixed in revision 2; while applying M-1 the author also moved
`RecoveryId` allocation into Task 3's incident module, so Task 3 does not
depend on Task 4's arbiter.

---

## Verdict

**2 blocking, 4 major, 1 minor.**  
Coverage: COMPLETE FOR DECLARED SCOPE.

This is a design review. It does not establish that the plan compiles, its tests pass, or implementation is approved.

## Incorporation audit

| Prior finding | Status |
| --- | --- |
| No prior review was supplied. | Check 1 skipped. |

## Findings

### Blocking

**B-1 — A superseding transition can advance before safety actions are acknowledged.** The plan says the arbiter never assumes an action succeeded, but its specified acknowledged outcomes cover commit results; it does not require receipts for admission closure, cancellation, Present terminalization, or quarantine transfer before the winner proceeds ([plan:49](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-23-phase-c0-stage-3a-i-plan-pure-arbiter.md:49), [plan:145](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-23-phase-c0-stage-3a-i-plan-pure-arbiter.md:145), [plan:167](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-23-phase-c0-stage-3a-i-plan-pure-arbiter.md:167)). If removal supersedes pre-submit DPMS, ordered actions alone allow the winner to start while cancellation or quarantine transfer remains incomplete. C.0 requires those obligations and forbids the displaced transition from opening or publishing state ([C.0:801](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:801), [C.0:3005](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:3005)). Add a pending phase that consumes acknowledgments of safety-critical actions before emitting actions that advance the winner; test delayed and failed acknowledgments.

**B-2 — The first completion-loss incident has no specified creation path.** Task 4 defines a fate function *for an active incident*, and its tests begin with an existing incident or attempt. Task 3 sends completion loss to `Poisoned` without specifying allocation of the initial `RecoveryId` ([plan:167](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-23-phase-c0-stage-3a-i-plan-pure-arbiter.md:167), [plan:190](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-23-phase-c0-stage-3a-i-plan-pure-arbiter.md:190), [plan:211](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-23-phase-c0-stage-3a-i-plan-pure-arbiter.md:211)). A normal-operation completion loss with no prior incident could therefore enter `Poisoned` without the single incident that later recovery and duplicate-event absorption must use. C.0 requires exactly one `RecoveryId` at that boundary ([C.0:768](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:768), [C.0:873](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:873)). Specify the creation input, ownership, and allocation rule; test first loss followed by duplicate recovery events.

### Major

**M-1 — Task 3 depends on Task 4’s recovery decisions.** Task 3 must select `NormalRecovery` and record a `REC-6` outcome on completion loss, but the matrix supplying those decisions is delivered in Task 4 ([plan:155](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-23-phase-c0-stage-3a-i-plan-pure-arbiter.md:155), [plan:167](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-23-phase-c0-stage-3a-i-plan-pure-arbiter.md:167), [plan:190](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-23-phase-c0-stage-3a-i-plan-pure-arbiter.md:190)). Task 3 can pass its gate with provisional fate logic that Task 4 later replaces. Move the matrix before the arbiter, or explicitly make Task 3’s gate cover the final matrix.

**M-2 — Arrival-order evidence does not establish item 63.** Pair elections, a third-kind transition-count check, and a storm boundedness check do not assert the final targets, dispositions, and convergence order after multiple lower-priority arrivals and replacements ([plan:130](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-23-phase-c0-stage-3a-i-plan-pure-arbiter.md:130), [plan:179](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-23-phase-c0-stage-3a-i-plan-pure-arbiter.md:179)). An arbiter could retain bounded state yet lose an administrative reprobe when later topology and DPMS generations arrive during an active higher transition. C.0 item 63 requires every lower-priority permutation to preserve and converge its valid representatives ([C.0:3045](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:3045)). Add a named mixed-arrival test that checks the exact ledger and successive winners, with a mutation that drops a lower field.

**M-3 — Item 65’s ordinary-work tag has no named evidence.** The plan states that ordinary work carries the epoch and no transition id, but its value-type task defines a `TransitionTag` with a required transition id, and its epoch tests cover bump order only ([plan:84](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-23-phase-c0-stage-3a-i-plan-pure-arbiter.md:84), [plan:161](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-23-phase-c0-stage-3a-i-plan-pure-arbiter.md:161), [plan:186](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-23-phase-c0-stage-3a-i-plan-pure-arbiter.md:186)). The existing owner record has an optional transition id, but the declared item-65 exit criterion still lacks a named test and mutation for ordinary work and its delayed reply ([C.0:3057](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:3057)). Add that evidence or state precisely which part of item 65 is gated in 3a-ii.

**M-4 — Poisoned DPMS has no disposition rule.** The only named Poisoned test checks that no KMS mutation action is emitted ([plan:174](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-23-phase-c0-stage-3a-i-plan-pure-arbiter.md:174), [plan:188](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-23-phase-c0-stage-3a-i-plan-pure-arbiter.md:188)). It would pass if the logical update also marked the device representative `Applied`, allowing the coordinator to count an unretired projection toward global completion. C.0 permits logical change while Poisoned but requires projection retirement for `Applied` ([C.0:855](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:855), [C.0:2051](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:2051)). Specify the truthful disposition and add a test that mutates it to `Applied`.

### Minor

**m-1 — A replaced deferred representative has conflicting endpoint rules.** R5-2 requires a displaced newer-generation representative to become `SupersededBy(newer)`, while R5-3 lists only `Applied`, `AbsorbedBy*`, or `Invalidated` as exits from `Deferred`; its test checks only that the newer-generation path is terminal ([plan:112](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-23-phase-c0-stage-3a-i-plan-pure-arbiter.md:112), [plan:120](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-23-phase-c0-stage-3a-i-plan-pure-arbiter.md:120), [plan:135](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-23-phase-c0-stage-3a-i-plan-pure-arbiter.md:135)). C.0’s typed latest-wins rule calls for `SupersededBy` ([C.0:867](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:867)). Resolve the wording and assert the exact disposition in that test.

## Coverage and implementation checks

Checks 2–4 covered the pure-layer ownership and handoffs, incident and disposition safety, the relevant `REC-1/4/5/6` rules, and §16.2 items 57 and 63–67. I used **11/24 bounded excerpts**, including the relevant C.0 sections, both stage-3 designs, existing lifecycle identities, and the compile-fail harness. I verified the named feature flags and harness exist; I did not run any gate.

The effectful driver, hardware behavior, and executed 3b–3d transitions remain outside this review. Rust expressions, test compilation, mutation execution, formatting, clippy, and portability checks belong to implementation.
