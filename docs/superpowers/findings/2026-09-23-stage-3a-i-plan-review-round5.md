# Stage 3a-i plan — codex review, round 5

**Target:** revision 5 (`78d9cc58`). **Result:** 1 blocking, 0 major, 0 minor; coverage COMPLETE FOR DECLARED SCOPE (11/24). Trend: r1 2B 4M 1m, r2 4B 1M, r3 3B 1M, r4 1B 1M, r5 1B. Incorporation: round-4 M-1 APPLIED, B-1 PARTIAL; no regression found.

**Reviewer:** `codex exec --sandbox read-only`, single pass; instrument `docs/superpowers/review/` @ `0245f96b`; `gpt-6-sol` `xhigh`; `codex-cli 0.155.1`.

**Author verification (2026-09-23):** CONFIRMED — revision 5 said the reported event "is the new incident's representative", which is an identity, not one of `REC-5`'s terminal dispositions (C.0 line 879), so a representative could stay unsettled after its incident resolved. Fixed in revision 6 (U-1b): the representative is pending while its incident lives and ends exactly once when the incident resolves. `RecoveryFailed` has no matching reason in C.0's `Invalidated` list (line ~879: shutdown, identity loss, output removal, newer generation), so revision 6 adds `Invalidated(RecoveryFailed)` as an explicit, stated extension of that list rather than forcing an unrelated reason.

---

## Verdict

**1 blocking, 0 major, 0 minor.**  
Coverage: **COMPLETE FOR DECLARED SCOPE**. This is a design review; it does not establish that code compiles, tests pass, or implementation is approved.

## Incorporation audit

| Round 4 finding | Status | Assessment |
| --- | --- | --- |
| B-1: teardown loss event has no terminal disposition | **PARTIAL** | [U-1a](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-23-phase-c0-stage-3a-i-plan-pure-arbiter.md:220) now assigns `Invalidated` to a reported loss during VT release, removal, or shutdown when no incident exists. Its broader claim that the reported event ends in *every* Table U row remains unsupported; see B-1 below. |
| M-1: table tests miss the coordinator-to-arbiter handoff | **APPLIED** | [Task 5](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-23-phase-c0-stage-3a-i-plan-pure-arbiter.md:380) adds a generated test that delivers a loss through the coordinator and arbiter during every active row and checks the selected actions and fate. |

The plan retains the earlier-round corrections for current-row selection, receipt ownership, DPMS-on resume, and mixed-arrival convergence. I found no separate regression in those contracts.

## Findings

### Blocking

**B-1 — An incident representative is not a terminal event disposition.** A normal-live completion loss gives event E to a new incident as its representative ([plan lines 199–200, 220–238](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-23-phase-c0-stage-3a-i-plan-pure-arbiter.md:199)). If that incident’s sole attempt fails, the plan moves the *incident* to `RecoveryFailed` ([plan line 255](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-23-phase-c0-stage-3a-i-plan-pure-arbiter.md:255)), but gives E no terminal `Disposition`. The same gap affects a first incident created during DPMS or a same-identity rebuild. Being a representative is an identity relationship, not one of C.0’s terminal outcomes. E can therefore remain unsettled after failure, contrary to [C.0 REC-5’s exactly-one-terminal rule](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:879) and the [row-completion rule](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:2036). Define the representative event’s exact disposition after both successful and failed incident resolution, then assert those values in the first-loss and handoff tests. The existing [handoff test](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-23-phase-c0-stage-3a-i-plan-pure-arbiter.md:380) cannot verify an outcome the table has not specified.

### Major

None.

### Minor

None.

## Coverage and implementation checks

- **Incorporation:** Audited both round 4 findings against revision 5 and checked the earlier corrections stated in the plan.
- **Architecture:** Checked coordinator, arbiter, recovery-table, and receipt responsibilities. The existing owner route processes a device’s event batch before admission wake ([source](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/backend.rs:20884)); this does not prove the future driver.
- **Safety and specification:** Checked event identity and disposition, recovery budget, supersession, quarantine handoffs, C.0 §6.4 REC-1–6, the §10 Table U rows, and §16.2 items 57 and 63–67. B-1 is the unresolved event-fate contract.
- **Limits:** **11/24** bounded spec/source excerpts used. Production driver behavior, physical barriers, and deferred 3a-ii or later execution remain unassessed. The plan assigns formatting, CI-form clippy, tests, and mutation checks to implementation; none were run.
