# Stage 3a-i plan — codex review, round 4

**Target:** revision 4 (`3419c28d`). **Result:** 1 blocking, 1 major, 0 minor; coverage COMPLETE FOR DECLARED SCOPE (15/24). Trend: r1 2B 4M 1m, r2 4B 1M, r3 3B 1M, r4 1B 1M. Incorporation: all four round-3 findings APPLIED.

**Reviewer:** `codex exec --sandbox read-only`, single pass; instrument `docs/superpowers/review/` @ `0245f96b`; `gpt-6-sol` `xhigh`; `codex-cli 0.155.1`.

**Author verification (2026-09-23):** both CONFIRMED — Table U's teardown rows named the fate of an *existing* incident only, so the reported loss event itself had none (C.0 lines 897–899: every event id reaches one terminal disposition; 2036–2042: a row completes only when every contributing event is settled) (B-1); the tables were tested directly but not through the coordinator → arbiter handoff (M-1). Fixed in revision 5.

---

## Verdict

**1 blocking, 1 major, 0 minor.** Coverage: **COMPLETE FOR DECLARED SCOPE**. This is a design review; it does not establish that the plan compiles, tests pass, or is approved for implementation.

## Incorporation audit

| Round 3 finding | Status | Assessment |
| --- | --- | --- |
| B-1: stale receipt credited to the new winner | **APPLIED** | The winner now requests its own transfer and waits for its own tagged receipt; a late receipt cannot open its gate ([plan:272–289](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-23-phase-c0-stage-3a-i-plan-pure-arbiter.md:272), [plan:325](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-23-phase-c0-stage-3a-i-plan-pure-arbiter.md:325)). |
| B-2: ordinary work selects the wrong loss row | **APPLIED** | The row active when loss is observed now decides, including for ordinary work ([plan:192–200](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-23-phase-c0-stage-3a-i-plan-pure-arbiter.md:192), [plan:252](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-23-phase-c0-stage-3a-i-plan-pure-arbiter.md:252)). |
| B-3: first DPMS-on loss pauses forever | **APPLIED** | A new incident is paused only with an off target; the on case has its own test ([plan:209–213](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-23-phase-c0-stage-3a-i-plan-pure-arbiter.md:209), [plan:253](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-23-phase-c0-stage-3a-i-plan-pure-arbiter.md:253)). |
| M-1: rebuild loss with no incident has no ID provenance | **APPLIED** | That branch now uses a coordinator event ID, creates one incident, and specifies its attempt ([plan:200–206](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-23-phase-c0-stage-3a-i-plan-pure-arbiter.md:200), [plan:254](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-23-phase-c0-stage-3a-i-plan-pure-arbiter.md:254)). |

## Findings

### Blocking

**B-1 — A loss during teardown gets an event ID but no defined terminal disposition.** Every reported completion loss receives a coordinator-allocated `NormalRecovery` event ID ([plan:217–223](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-23-phase-c0-stage-3a-i-plan-pure-arbiter.md:217), [plan:351–355](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-23-phase-c0-stage-3a-i-plan-pure-arbiter.md:351)). If an ordinary commit becomes unknown during `VTRelease` with no prior incident, Table U correctly creates no incident, but specifies only the invalidation of *an existing* incident; it gives the new event ID no fate ([plan:192–200](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-23-phase-c0-stage-3a-i-plan-pure-arbiter.md:192)). The same gap applies to removal and shutdown. The row can then finish with an unterminated event, or leave a recovery intent for later convergence. C.0 requires every event ID to reach one terminal disposition and every contributing event to be settled before row completion ([C.0:897–899](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:897), [C.0:2036–2042](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:2036)). Specify and test the reported event’s terminal disposition in each no-incident Table U row, including its path through the arbiter.

### Major

**M-1 — Table tests do not establish that the arbiter selects Table U at the loss boundary.** Task 3 tests Table U directly, while Task 4’s acknowledged-outcome test exercises only DPMS and Task 5’s loss test proves ID allocation ([plan:248–254](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-23-phase-c0-stage-3a-i-plan-pure-arbiter.md:248), [plan:305–309](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-23-phase-c0-stage-3a-i-plan-pure-arbiter.md:305), [plan:332](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-23-phase-c0-stage-3a-i-plan-pure-arbiter.md:332), [plan:364](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-23-phase-c0-stage-3a-i-plan-pure-arbiter.md:364)). For example, a loss observed during `VTRelease` could be projected as a lower-priority `NormalRecovery` event without the arbiter applying VT release’s loss row; the table tests would still pass. C.0 assigns the outcome to the row encountering `CompletionUnknown` ([C.0:2044–2055](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:2044)). Add pure coordinator-to-arbiter tests that deliver a loss during each active row and assert the selected actions, incident fate, and event disposition. This tests the decision-layer handoff without requiring the deferred production driver.

### Minor

None.

## Coverage and implementation checks

- **Incorporation:** All four round 3 corrections are present. Earlier fixes for prompt logical teardown, ordinary-work tags, poisoned DPMS deferral, and mixed arrivals remain in the plan text.
- **Architecture:** Checked coordinator, arbiter, recovery-table, and receipt handoffs against C.0 and the stage designs. The existing owner route processes event batches on the core loop ([source:20816–20886](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/backend.rs:20816)); it does not prove the future driver’s behavior.
- **Safety and specification:** Checked identity provenance, supersession, loss-row ordering, recovery budget, and the in-scope REC-1–6 and §16.2 requirements. The two findings identify the remaining event-fate contract and its verification gap.
- **Verification limits:** **15/24** bounded spec/source excerpts used. Production driver receipts, executor reap barriers, hardware behavior, and the deferred 3a-ii and later-stage execution remain unassessed, not presumed sound. The plan assigns formatting, CI-form clippy, tests, and compiled mutation checks to implementation; I ran none.
