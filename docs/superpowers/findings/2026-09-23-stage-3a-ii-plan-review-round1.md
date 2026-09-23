# Stage 3a-ii plan — codex review, round 1

**Target:** revision 1 (`a690ebc1`). **Result:** 2 blocking, 4 major, 0 minor; coverage COMPLETE FOR DECLARED SCOPE (24/24).

**Reviewer:** `codex exec --sandbox read-only`, single pass; instrument `docs/superpowers/review/` @ `0245f96b`; `gpt-6-sol` `xhigh`; `codex-cli 0.155.1`.

**Author verification (2026-09-23):** all six CONFIRMED against the plan text and the cited anchors — no path from a coordinator projection to the driver on an idle device (B-1); no pre-installation projection for a new output (B-2, umbrella rev-2 M-2); no delayed-executor supersession test (M-1, design §5.2); the core's early return at `process_request.rs:10290` would strand a completion-only queue under a per-CRTC answer (M-2); no destroy-while-off/while-lit comparison (M-3, design §3.5 rev 5); `clear_all_armed_vblank_targets` at `render/backend.rs:31090` is server-wide (M-4). Also taken from the review's coverage notes: Task 7's single mutation cannot prove a whole inventory, so each inventoried read now gets its own. Fixed in revision 2.

---

## Verdict

**2 blocking, 4 major, 0 minor.**  
Coverage: **COMPLETE FOR DECLARED SCOPE**. This is a design review; it does not establish that the plan compiles, passes tests, or is approved for implementation.

## Incorporation audit

| Prior review | Assessment |
| --- | --- |
| None | First review; check 1 skipped. |

## Findings

### Blocking

**B-1 — A DPMS request has no specified path that starts the driver.** The plan places the driver at `route_owner_event_batch` ([plan:27](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-23-phase-c0-stage-3a-ii-plan-dpms-execution.md:27)), while Task 4 sends Owner requests to the coordinator ([plan:130](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-23-phase-c0-stage-3a-ii-plan-dpms-execution.md:130)). It specifies no call or scheduled wake connecting that projection to the driver. On an idle device, `set_dpms_power(off)` can return after recording the request with no owner event to route, so no commit starts. The spec requires hardware completion to follow through the driver ([spec:143](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-23-phase-c0-stage-3a-arbiter-and-dpms-design.md:143), [spec:264](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-23-phase-c0-stage-3a-arbiter-and-dpms-design.md:264)). Specify an immediate driver kick after projection, plus receipt driven reentry, and name a test where no unrelated owner event occurs.

**B-2 — New outputs can miss an existing global off target.** Task 4 projects onto current outputs, but no task refreshes a newly installed output from the coordinator’s current level before installation ([plan:120](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-23-phase-c0-stage-3a-ii-plan-dpms-execution.md:120)). After global off, an output introduced by fixture topology work could be installed active. The umbrella requires the refresh rule and its **3a test**, even though executed hotplug belongs to 3c ([umbrella:232](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-22-phase-c0-stage-3-lifecycle-design.md:232)); the authoritative spec also names that fixture evidence ([spec:379](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-23-phase-c0-stage-3a-arbiter-and-dpms-design.md:379)). Add the preinstallation projection hook and a test that fails if a new output inherits an on target.

### Major

**M-1 — Supersession evidence does not hold a delayed executor call.** D2 checks receipt tags and D6 checks stale results, but neither named scenario asserts that the winning transition waits while the earlier executor call is outstanding ([plan:56](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-23-phase-c0-stage-3a-ii-plan-dpms-execution.md:56), [plan:79](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-23-phase-c0-stage-3a-ii-plan-dpms-execution.md:79)). If off’s host call is delayed and on supersedes it, early slot reuse can dispatch on before off returns. The spec explicitly requires this sequence ([spec:400](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-23-phase-c0-stage-3a-arbiter-and-dpms-design.md:400)). Add a delayed executor fixture test asserting the winner stays queued until return or reap, with premature dispatch as its mutation.

**M-2 — Blackout tests miss a completion-only queue.** The current core pass returns when `present_pending_exec` is empty and its global blackout answer is false ([source:10290](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver-core/src/core_loop/process_request.rs:10290)). Task 6 changes blackout to per CRTC, but its tests use future-target Presents and do not isolate an off CRTC’s parked completion after its execution queue empties ([plan:161](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-23-phase-c0-stage-3a-ii-plan-dpms-execution.md:161)). That completion can remain behind a frozen clock. The spec also requires while-off Presents to deliver completion, `IdleNotify`, and release exactly once ([spec:391](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-23-phase-c0-stage-3a-arbiter-and-dpms-design.md:391)). Add a completion-only off-CRTC test covering those outcomes and a mutation that restores the early return.

**M-3 — Post-on unflip equivalence has no named test.** Task 5 proves one destroy-while-off case eventually unflips ([plan:158](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-23-phase-c0-stage-3a-ii-plan-dpms-execution.md:158)). It does not compare readiness and outcome with destroy-while-lit, which the spec expressly requires, including treatment of the no-composed-return case ([spec:389](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-23-phase-c0-stage-3a-arbiter-and-dpms-design.md:389), [spec:483](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-23-phase-c0-stage-3a-arbiter-and-dpms-design.md:483)). DPMS could alter an unflip readiness input yet still pass the eventual-unflip test. Add the paired comparison and establish the accepted Ciii addendum as its prerequisite.

**M-4 — The Legacy-step inventory lacks a check for Owner vblank arms.** Task 4 names vblank-target clearing for inventory, but its mixed-server tests detect Legacy loop leakage and Owner scanout resets, not loss of an Owner arm ([plan:125](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-23-phase-c0-stage-3a-ii-plan-dpms-execution.md:125), [plan:135](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-23-phase-c0-stage-3a-ii-plan-dpms-execution.md:135)). Today Legacy off calls `clear_all_armed_vblank_targets` ([source:31090](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/backend.rs:31090)). If that remains global, a Legacy off can erase a lit Owner CRTC’s timing arm, contrary to the required device scoping ([spec:123](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-23-phase-c0-stage-3a-arbiter-and-dpms-design.md:123)). Add a named mixed-server test that preserves an Owner arm across Legacy off, with global clearing as its mutation.

### Minor

None.

## Coverage and implementation checks

Checks 2–4 covered the driver trigger, task order, commit and result contracts, failure timing, mixed-device state, Present sweeps, and named evidence. **24/24 bounded excerpts** were used across specs and source; the plan was read once. Task 2 and Task 3 precede Task 5’s dependencies. The driver and device commit owner have distinct stated roles; the C.0 deadline bootstrap correction matches the current authority ([C.0:2218](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:2218)). Nine tasks fit the umbrella’s approximate fourteen-task limit.

Task 7 requires a file-and-line inventory, but this review did not audit every `kms_outputs_active` read; its single chosen mutation cannot establish coverage of every inventoried path. The hardware test is specified but unrun, so it supplies no hardware result here. Exact Rust interfaces, portability, compilation, formatting, clippy, and test outcomes remain implementation checks under the plan’s gate.
