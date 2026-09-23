# Stage 3a-ii plan — codex review, round 2

**Target:** revision 3 (`e95966ee`, anchors re-pointed after merge `a232d2af`). **Result:** 0 blocking, 2 major, 0 minor; coverage INCOMPLETE (24/24; the full §3.4 commit shape, the §3.8 read inventory, the deadline bootstrap and the merged COW/timestamp behaviour not assessed). Trend: r1 2B 4M, r2 0B 2M. Incorporation: all six round-1 findings APPLIED.

**Reviewer:** `codex exec --sandbox read-only`, single pass; instrument `docs/superpowers/review/` @ `0245f96b`; `gpt-6-sol` `xhigh`; `codex-cli 0.155.1`.

**Author verification (2026-09-23):** both CONFIRMED — a pre-IPC refusal calls `route_owner_event_batch` synchronously (`render/admission.rs:2279`), so a driver kicked inside `set_dpms_power` could re-enter itself (M-1); the core emits `DPMSInfoNotify` to subscribed clients (`yserver-core/src/core_loop/process_request.rs:8933`), which the 3a design §4 and the umbrella's table had denied — they are corrected in the same commit as plan revision 4 (M-2). The Legacy golden's "DPMS emits no RANDR event" stays true: its listener selected only RANDR.

---

## Verdict

**0 blocking, 2 major, 0 minor.**  
Coverage: **INCOMPLETE**. This is a design-review result; it does not establish that the plan compiles, passes tests, or is approved for implementation.

## Incorporation audit

| Round-one finding | Assessment |
| --- | --- |
| B-1, idle DPMS never starts the driver | **APPLIED.** Task 1 now requires a kick from the projection and tests an idle device. The synchronous form introduces the separate delivery-contract gap M-1 below. |
| B-2, new output misses global off | **APPLIED.** Task 4 requires projection before installation and names a test through the production registration path. The current tree has a connector reconciliation path before relight; executed topology paths remain for 3b/3c. |
| M-1, delayed executor supersession | **APPLIED.** Task 2 holds the earlier call and asserts that the winner waits for its return or reap. |
| M-2, completion-only blackout queue | **APPLIED.** Task 6 names that queue, its completion, `IdleNotify`, release, and the early-return mutation. |
| M-3, destroy-while-off equivalence | **APPLIED.** Task 5 compares the off and lit cases and names the accepted Ciii addendum as a prerequisite. |
| M-4, Owner vblank arm cleared by Legacy off | **APPLIED.** Task 4 adds a mixed-server arm-preservation test and mutation. |

Task 7 also replaces the earlier single inventory mutation with one mutation per inventoried `kms_outputs_active` read.

## Findings

### Blocking

None demonstrated within the examined ground.

### Major

**M-1 — The immediate driver kick lacks a nonreentrant delivery contract.** [Task 1](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-23-phase-c0-stage-3a-ii-plan-dpms-execution.md:58) permits actions and receipts to feed back into the arbiter in the same `set_dpms_power` call. The core calls that backend hook from its DPMS request handler ([source](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver-core/src/core_loop/process_request.rs:8928)). A conductor refusal can synchronously call `route_owner_event_batch` ([source](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/admission.rs:2274)); Task 1 places the driver at that routing site. Thus an immediate topology dispatch that refuses can enter the driver again before its first action batch finishes. The plan gives no ordering rule for actions and receipts across that entry, or a bound on work done before the hook returns. Specify a per-device run-to-completion queue for receipts and actions, or a scheduled, nonreentrant kick; preserve the spec’s return-after-projection contract ([spec](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-23-phase-c0-stage-3a-arbiter-and-dpms-design.md:149)). Test the synchronous refusal path, since the idle-device test alone does not exercise it.

**M-2 — The protocol differential assumes a listener can receive no DPMS event.** Task 9 says “DPMS has no event” and treats any listener byte as a defect ([plan](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-23-phase-c0-stage-3a-ii-plan-dpms-execution.md:252)). The current core emits `DPMSInfoNotify` after a changed level to subscribed clients ([source](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver-core/src/core_loop/process_request.rs:8933)). If the listener subscribes, the proposed assertion rejects existing behavior; if it does not, the test cannot compare the event contract seen by subscribed clients. Specify both listener states and compare Legacy and Owner bytes for each, including the event when subscribed. The spec requires the immediate protocol level to remain unchanged ([spec](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-23-phase-c0-stage-3a-arbiter-and-dpms-design.md:149)).

### Minor

None.

## Coverage and implementation checks

**24/24 bounded spec and source excerpts used.** Incorporation was checked against every round-one finding. Architecture and safety checks verified the core DPMS hook, synchronous refusal route, Owner event routing, connector reconciliation before relight, and the fact that a platform enable commits a modeset before appending the active output. Task 4’s preinstallation requirement is therefore consequential; the plan supplies it, while 3b/3c retain responsibility for proving their executed installation sites.

For Present, the current early return and completion sweep were checked against Task 6. Its completion-only test addresses the prior gap, and its per-target rule retains Legacy’s stated all-or-nothing blackout answer. The remaining spec and baseline ground was **not established as sound**: the excerpt limit prevented a direct assessment of the full §3.4 commit shape, §3.8 read inventory, C.0 deadline bootstrap, and the merged COW and compose-timestamp behavior. Those are the specific scope for any follow-up.

The plan assigns formatting, all-targets Clippy, tests, release checks, mutations, and an unrun hardware test to implementation. This review ran none of them and makes no hardware or compiler claim.
