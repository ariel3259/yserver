# Stage 3a-i plan — codex review, round 3

**Target:** revision 3 (`76e6e39e`). **Result:** 3 blocking, 1 major, 0 minor;
coverage COMPLETE FOR DECLARED SCOPE (18/24). Trend: r1 2B 4M 1m, r2 4B 1M,
r3 3B 1M. Incorporation: B-3, B-4, M-1 APPLIED; B-2 PARTIAL; B-1 TRADED.

**Reviewer:** `codex exec --sandbox read-only`, single pass; instrument
`docs/superpowers/review/` @ `0245f96b`; `gpt-6-sol` `xhigh`; `codex-cli 0.155.1`.

**Author verification (2026-09-23):** all four CONFIRMED against the plan text
and C.0 — re-attributing a receipt certified a transfer that was never made to
the winner (B-1, C.0 lines 801–820); "or ordinary work" contradicted "the active
row decides" (B-2, lines 2044–2050); a DPMS-on loss created a paused incident
with the target on, while `REC-6` defers only "on `dpms_target = On`" (B-3,
line 928); the rebuild row speaks only of an existing incident (M-1, line 2052).
Each fixed directly in revision 4; no section needed rewriting.

---

## Verdict

**3 blocking, 1 major, 0 minor.** Coverage: **COMPLETE FOR DECLARED SCOPE**. This is a design review; it does not establish that the plan compiles, tests pass, or implementation is approved.

## Incorporation audit

| Round 2 finding | Status | Assessment |
| --- | --- | --- |
| B-1: receipts block logical teardown | **TRADED** | [R4-2a](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-23-phase-c0-stage-3a-i-plan-pure-arbiter.md:255) now emits logical actions immediately, but treats a former winner’s receipt as proof for the current winner (B-1 below). |
| B-2: generic completion-loss fate | **PARTIAL** | Table U separates VT release, removal, and shutdown, but its “or ordinary work” clause can select normal recovery while one of those rows is active (B-2). |
| B-3: first incident lacks an event ID | **APPLIED** | U-3 and C-5 assign the loss a coordinator event ID and make it the incident representative. |
| B-4: poisoned DPMS-on cannot resume | **APPLIED** | F-2 and R4-7 resume an *existing* paused incident on logical DPMS-on. The first-loss DPMS-on case has a separate defect (B-3). |
| M-1: mixed arrivals cover only VT release | **APPLIED** | The generated test now covers every active kind, exact dispositions, and successive winners. |

Round 1’s matrix ordering, ordinary-work tag, poisoned projection disposition, and replaced-`Deferred` corrections remain present. Its supersession and first-loss risks carry forward through the round 2 findings above.

## Findings

### Blocking

**B-1 — A stale transfer receipt cannot certify the current winner’s quarantine.** The plan says a receipt tagged to a superseded transition is “re-attributed” to the current winner, and its test requires that behavior ([plan:255–270](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-23-phase-c0-stage-3a-i-plan-pure-arbiter.md:255), [plan:306](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-23-phase-c0-stage-3a-i-plan-pure-arbiter.md:306)). Suppose VT release requests a quarantine transfer, shutdown supersedes it, and the VT receipt arrives afterward. That receipt proves a transfer to VT release, not to shutdown; counting it toward shutdown can open the physical gate while resources remain with the former winner. C.0 requires quarantine ownership to follow the winner, including for stale results ([C.0:801–820](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:801)). Require a transfer and acknowledgment for the current winner, or specify a resource-level proof that remains valid across winners. Change the late-receipt test to reject a merely retagged transfer.

**B-2 — “Ordinary work” can select the wrong CompletionUnknown row.** U-1 defines normal live operation as “no lifecycle transition active, **or ordinary work**,” although the task says Table U is selected by the active row ([plan:168–189](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-23-phase-c0-stage-3a-i-plan-pure-arbiter.md:168)). An ordinary Present can remain outstanding when VT release starts; if it becomes unknown during that drain, the “ordinary work” branch creates a recovery incident instead of following VT release’s no-incident, immediate-seat-release fate. C.0 assigns the fate to the transition encountering the loss ([C.0:2044–2050](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:2044)). Make the active row authoritative at observation time. Test an ordinary commit becoming unknown during VT release, removal, and shutdown.

**B-3 — First loss during DPMS-on can strand a paused incident.** U-1 creates an incident on DPMS completion loss and “at once” pauses it, without distinguishing off from on ([plan:195–199](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-23-phase-c0-stage-3a-i-plan-pure-arbiter.md:195)). With no prior incident, a DPMS-on commit that becomes unknown therefore creates a paused incident whose target is already on; no later event need arrive to resume its sole attempt. The plan’s own F-2 and C.0 require logical DPMS-on to resume a paused attempt, while DPMS-on itself supplies no new recovery authority ([plan:217–223](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-23-phase-c0-stage-3a-i-plan-pure-arbiter.md:217), [C.0:928](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:928), [C.0:2051](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:2051)). Specify the first-loss DPMS-on fate and ensure an on target does not leave an authorized incident paused. Test it separately from resuming a pre-existing incident.

### Major

**M-1 — Same-identity topology with no incident has no defined RecoveryId provenance.** U-1 says that when no incident exists, the rebuild’s “own `RecoveryId` outcome” applies to its single attempt ([plan:192–195](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-23-phase-c0-stage-3a-i-plan-pure-arbiter.md:192)). C.0 specifies transfer **when an incident exists**; REC-6’s topology outcome likewise concerns an existing incident ([C.0:2052](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:2052), [C.0:919–927](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:919)). If the first loss occurs during such a rebuild, the plan does not say whether it allocates a first incident or has a different no-incident fate, yet its test claims an exact outcome for that case ([plan:234](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-23-phase-c0-stage-3a-i-plan-pure-arbiter.md:234)). Define that branch’s event-ID and RecoveryId authority, state, and attempt budget before implementation; make the test assert them.

### Minor

None.

## Coverage and implementation checks

I read the plan and both review rounds once and used **18/24 bounded excerpts**: 15 from the relevant specs and three from existing source. The source confirms the current owner-event route and DPMS entry point; it does not prove the future driver’s receipt behavior. I checked incorporation, pure-layer ownership and handoffs, failure ordering, and the proposed evidence for C.0 §16.2 items 57 and 63–67. The named table and mutation tests cover much of that scope, but their present expectations cannot establish the four contracts above.

The plan assigns nightly formatting, CI-form clippy, tests, and compiled mutation checks to implementation. I ran none. The 3a-ii driver, hardware behavior, and executed later-stage transitions remain unassessed, not presumed sound.
