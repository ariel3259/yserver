# Stage 3d-i-1 plan — review round 2

**Result:** 0 blocking, 1 major, 0 minor; coverage INCOMPLETE (24/24). **Loop closed.**

**Reviewer:** `codex exec --sandbox read-only`, single pass
**Instrument:** `yserver-review` @ `111490f`; model `gpt-6.1-sol`; reasoning effort `xhigh`; `codex-cli 0.159.2`.

**Coordinator classification:** M-2 (one hardware cycle at Task 6 instead of 4) is an evidence detail and
the open question (retained old-executor reply draining) is integration wiring: no design defect, so the
loop closes (user's convergence rule). Both folded into plan revision 3 (Task 6 four cycles; IN-1 in
Task 8).

## Verdict

0 blocking, 1 major, 0 minor

Coverage: INCOMPLETE

This is a design-review result, not implementation approval or evidence that code compiles or tests pass.

References: **P** = [target plan](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-10-04-phase-c0-stage-3d-i-1-plan-recovery-core.md); **S** = [authoritative spec](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-10-04-phase-c0-stage-3d-recovery-quarantine-shutdown-design.md). Line numbers refer to HEAD `27ccf536`.

## Incorporation audit

| Prior finding | Disposition | Assessment |
| --- | --- | --- |
| B-1: core-blocking preparation probe | APPLIED | Decision 4 and Task 5 move forced discovery onto a tracked worker, consume its tagged result on the core, remove the preparation’s test fork, and test responsiveness and supersession (P:77–86, 187–201). |
| M-1: production completion-loss routing | APPLIED | Decision 6 assigns one terminal-result route. Task 6 injects lost completion through the fresh executor/core path and checks `RecoveryFailed`, the same incident, one withdrawal and no retry (P:90–97, 218). |
| M-2: hardware gates depend on future deliverables | PARTIAL | Tasks 1 and 3 introduce incremental hardware cases; Task 4 extends the available case; Task 5 assigns coordinator VT evidence; the recovery case first runs in Task 6. However, Task 6 explicitly limits that run to one cycle, below the spec’s minimum. See M-2 below. |
| m-1: conflated control and family closure | APPLIED | Task 1 distinguishes holder release, control/accounted-alias discharge, and the registry’s final close. Its mutations now target premature control closure or treating that closure as sufficient proof (P:119–122, 133). |

## Findings

### Blocking

None established.

### Major

**M-2 — Task 6’s hardware gate under-runs the required recovery case**

P:220–221 explicitly schedules **one cycle** of `c0_hw_3di_completion_loss_recovers_on_card1_drm` after Task 6, postponing four-cycle acceptance to Task 9. S:320–324 requires at least four cycles of this hardware case, while S:273–275 and P:51–59 require hardware evidence at each task touching a real path.

Concrete consequence: Task 6 can advance after a single successful fresh-incarnation reinstall even if subsequent recovery cycles fail. Tasks 7–8 then build on qualification and admission reopening without Task 6 having satisfied its required hardware gate. Task 9’s later acceptance run cannot supply evidence at the earlier task boundary.

**Smallest correction:** deliver and run the existing recovery hardware case for at least four cycles in Task 6, using only Tasks 1–6. Task 9 can retain its final acceptance run and coverage-recording work. This concerns the successful recovery case, not the explicitly deferred failure-path hardware.

### Minor

None established.

## Coverage and implementation checks

1. **Incorporation audit:** completed for all four prior findings. Three corrections are applied; the per-task hardware correction remains partial.

2. **Architecture and cross-task contracts:** assessed incident allocation, authoritative outcome routing, task order, fresh-entry construction, worker-result consumption and acquire-preparation reuse. The scripted Task 2 seam appropriately precedes real reopen/reinstall delivery. **Retired-executor reply delivery remains unresolved**, as detailed below.

3. **Safety, ownership and failure semantics:** assessed reap/join requirements, stop-alias-creation ordering, control versus final closure, fresh-incarnation identity, current-transition checks, supersession and late-result obligations. No additional unsafe sequence was established within the bounded investigation. Buffered replies across reap/replacement remain unassessed.

4. **Spec compliance and verification:** assessed the declared decisions, recovery flow, arbiter interaction, lease holders and evidence rules. A/B/F are stated globally; incremental C evidence is substantially repaired, with M-2 remaining. P:264–270 assigns formatting, regular all-target Clippy, feature variants, tests and load/mutation execution. Actual results and applicable Linux glibc/musl/FreeBSD buildability remain implementation checks.

**Reading budget: 24/24 bounded excerpts**—four spec excerpts and twenty source excerpts, excluding the single reads of the plan and prior review. HEAD was verified. No builds, tests, benchmarks, installs, compilation experiments or additional reviewers ran.

**Specific unresolved question:** which core-loop owner drains buffered old-executor replies and preserves their request/transition correlation across reap, movement into quarantine and fresh-incarnation installation?

P:124 detaches event readers, P:159–161 moves old objects into quarantine, and P:246 requires an old `Accepted` result to be consumed after `Ready`. The inspected watchdog pump traverses active device entries ([platform.rs:6059](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/platform.rs:6059)); executor ticking clears `in_flight` after reap ([executor/mod.rs:1082](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/executor/mod.rs:1082)). Those excerpts do **not** establish whether another path preserves correlation and drains retained transports. A bounded follow-up should inspect only IPC reply routing and retained-executor progress against S:136–144.

Explicitly deferred 3d-i-2 and 3d-ii work was excluded. Neither those exclusions nor the unresolved reply-delivery question are claims of soundness.