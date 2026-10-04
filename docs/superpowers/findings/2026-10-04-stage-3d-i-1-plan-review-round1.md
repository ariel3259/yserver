# Stage 3d-i-1 plan — review round 1

**Result:** 1 blocking, 2 major, 1 minor; coverage COMPLETE FOR DECLARED SCOPE (22/24).

**Reviewer:** `codex exec --sandbox read-only`, single pass
**Instrument:** `yserver-review` @ `111490f`; model `gpt-6.1-sol`; reasoning effort `xhigh`; `codex-cli 0.159.2`.

**Coordinator verification:** all confirmed. B-1: `lifecycle_prepare_acquire_topology` reaches
`get_connector(connector, true)` on the core (`modeset.rs` ~720) and carries a `#[cfg(not(test))]` fork in
its body — a pre-existing 3c defect on the VT acquire path too, fixed in the new Task 5. M-1, M-2, m-1:
design (routing contract, per-task hardware, closure sequence). Plan revision 2.

## Verdict

1 blocking, 2 major, 1 minor

Coverage: COMPLETE FOR DECLARED SCOPE

This is a design-review result, not implementation approval or evidence that code compiles or tests pass.

References below use **P** for the target plan and **S** for the authoritative spec. Source paths are relative to `crates/yserver/src`.

## Incorporation audit

| Prior review | Disposition |
| --- | --- |
| None | Check 1 skipped as instructed. |

## Findings

### Blocking

**B-1 — Reusing acquire preparation preserves a core-blocking connector probe**

P:70–72 and 153–155 require reuse of `lifecycle_prepare_acquire_topology`. Its production body synchronously calls `output_for_exact_probe_assignment` ([admission.rs:3272](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/admission.rs:3272)), which executes `get_connector(connector, true)` ([modeset.rs:720](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/drm/modeset.rs:720)). The existing admission dispatcher calls this preparation directly ([admission.rs:4323](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/admission.rs:4323)).

Concrete sequence: the old incarnation retires successfully; recovery opens the fresh incarnation; reinstall preparation enters a stuck `GETCONNECTOR`. The core cannot process VT release, deadlines or client requests. This violates S:131–135, including core responsiveness during recovery. Task 1’s stuck-worker test covers an **old** incarnation worker and does not establish safety of fresh preparation.

**Required correction:** explicitly split blocking discovery from core-owned preparation, using a tracked worker/helper and asynchronous result delivery. Retain its incarnation lease until join/reap, and revalidate transition/epoch before final validation and installation. Add a deterministic fresh-probe stall scenario proving core responsiveness and supersession.

### Major

**M-1 — Scripted failure evidence does not prove production completion-loss routing**

Task 2 tests `CompletionUnknown` through a scripted reopen/reinstall seam (P:113–122). After the real installation exists, Task 5 tests rejection, but no test sends an unknown completion through that installation’s production terminal-result path (P:157–161).

This matters because the existing installation reports unknown completion through `lifecycle_report_completion_loss` ([admission.rs:7067](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/admission.rs:7067)). For an attempting incident, Table U resolves the incident as failed ([recovery.rs:822](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/owner/lifecycle/recovery.rs:822)), while the arbiter’s generic loss handling sets the lifecycle to `Poisoned` ([arbiter.rs:1211](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/owner/lifecycle/arbiter.rs:1211)). A subsequent `FailedOrUnknown` attempt outcome requires both `Recovering(id)` and an `Attempting` incident ([arbiter.rs:1336](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/owner/lifecycle/arbiter.rs:1336)).

Thus preserving the generic route and adding a second attempt-outcome notification can leave the device `Poisoned` with an already-failed incident; the scripted test can still pass. S:116–120 and 279–281 require `RecoveryFailed`, withdrawal and no recursion.

**Required correction:** assign one authoritative routing contract for real recovery-install terminal results, including how generic Table U handling interacts with the attempt outcome. In Task 5 or later, inject unknown completion through the executor/core path on the fresh incarnation and assert `RecoveryFailed`, unchanged `RecoveryId`, one withdrawal and no retry. Failure-path hardware may remain deferred.

**M-2 — Per-task hardware gates depend on future deliverables**

P:44–52 requires hardware after every task touching a real path and prohibits tests depending on future tasks. S:273–275 imposes the same evidence rule.

Tasks 1, 3 and 4 change real lease, reopen and resource-registration paths. However, the only named, authorized hardware test is delivered in Task 8 and requires the complete poison → reopen → reinstall → compose sequence (P:188–200). It cannot establish those earlier task gates before Task 5 exists. P:6–9 also prevents substituting an unnamed hardware test.

Concrete consequence: execution must either advance those tasks without their required hardware evidence or implement later deliverables early, defeating the stated task dependency boundary.

**Required correction:** name and assign incremental hardware cases that exercise each real-path task using only available deliverables, with their end-state checks and mutations. Keep Task 8 as the complete recovery acceptance case.

### Minor

**m-1 — The closure mutation conflates control close with final family close**

Task 1 says the mint occurs before the old fd set is closed and names “close the owner fd before the mint” as the failing mutation (P:96–107). The named registry operation requires the control alias already closed ([drm_cleanup.rs:590](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/resources/drm_cleanup.rs:590)), then discharges payload aliases and performs its final close before returning proof (lines 609–632).

The plan does not distinguish these closes. An implementer interpreting the mutation as forbidding the required control close can keep the mint permanently unavailable; a test permitting only synthetic closure evidence would conceal that problem.

**Required clarification:** specify the sequence: release lease holders, close/discharge control and other accounted aliases, then mint proof through the registry’s final closure. Make the mutation target control closure **before holder release**, or treating control closure alone as family-closure proof. This preserves S:95–105 without expanding the deferred quarantine inventory.

## Coverage and implementation checks

- **Check 1:** skipped; no prior findings.
- **Check 2:** assessed task order, incident ownership, fresh-incarnation dependencies, preparation reuse and outcome delivery. Found the production routing and hardware dependency gaps.
- **Check 3:** assessed lease/reap/join barriers, reopen ordering, supersession, DPMS and late-result contracts. Verified worker identity/join accounting and registry mint preconditions; found the fresh-probe blocking defect and closure ambiguity.
- **Check 4:** assessed the declared recovery requirements and A/B/C/F evidence rules. End-state checks, core-driver execution and load/mutation gates are stated; production unknown-completion coverage and incremental hardware evidence are insufficient.

**Excerpts used: 22/24**, each at most 120 lines, excluding the single plan read. Source HEAD was verified as `2014c916`.

Deferred items remain excluded: recovery RANDR gating, administrative reprobe, round-5 integration questions, topology differential, failure-path hardware, quarantine inventory, Removed teardown and orderly shutdown. Their exclusion is not a finding or a claim of soundness.

No builds, tests or compilation experiments ran. Formatting, regular Clippy, feature/build checks, test execution, mutation results and applicable ioctl portability checks remain implementation work.