## Verdict

**1 blocking, 1 major, 0 minor**  
Coverage: COMPLETE FOR DECLARED SCOPE

This is a design review. It does not establish that the plan compiles, passes tests, or is approved for implementation.

## Incorporation audit

| Round 2 finding | Status | Assessment |
| --- | --- | --- |
| B-1, reset can lose an installed change | **APPLIED** | Task 2 now defers generation reset and `-terminate` until an install-capable mutation is terminal, then seeds from backend state; its named test covers the last-client case ([plan:112](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-24-phase-c0-stage-3b-ii-plan-randr-protocol.md:112), [plan:130](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-24-phase-c0-stage-3b-ii-plan-randr-protocol.md:130)). This matches [spec §7.2](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-24-phase-c0-stage-3b-modeset-and-randr-design.md:643). |
| M-1, too few requester connections | **APPLIED** | Task 5 now assigns three independent requesters and a listener to the concurrent and mixed-server scripts, with bytes checked per connection ([plan:181](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-24-phase-c0-stage-3b-ii-plan-randr-protocol.md:181)). |

## Findings

### Blocking

**B-1 — A forced resource query can publish a topology change outside the gate.** Task 1 says queries never block and read published state; Task 4 routes backend requester-less publications through the gate ([plan:73](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-24-phase-c0-stage-3b-ii-plan-randr-protocol.md:73), [plan:157](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-24-phase-c0-stage-3b-ii-plan-randr-protocol.md:157)). But `GetScreenResources` calls `reprobe_connectors` before replying ([process_request.rs:2939](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver-core/src/core_loop/process_request.rs:2939)); KMS then rebuilds RANDR state and emits connector notifications immediately if the probe finds a change ([backend.rs:24457](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/backend.rs:24457), [backend.rs:10348](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/backend.rs:10348)).

If Owner A has a dispatched modeset and B’s query discovers a change on another connector, B’s probe publishes and notifies before A reaches its terminal result. That bypasses the [spec §7.5 ordering rule](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-24-phase-c0-stage-3b-modeset-and-randr-design.md:724) for a physical topology change. Specify how a forced query separates sensing from publication: keep the query runnable against the currently published snapshot, and route any discovered publication through the gate after A’s terminal result and before the next waiter. Add a test exercising the forced-query path; the Task 4 test producer does not exercise it.

### Major

**M-1 — The wire differential has no pre-C.0 Legacy oracle.** Task 5 compares bytes from the Legacy and Owner fixtures built from the same modified core ([plan:181](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-24-phase-c0-stage-3b-ii-plan-randr-protocol.md:181)). Its exception-set assertion would still pass if a shared handler changed both fixtures’ reply or event bytes identically. The test therefore establishes relative parity, but cannot by itself establish the required parity with *yserver-before-C.0* Legacy. The [spec §8.2 evidence requirement](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-24-phase-c0-stage-3b-modeset-and-randr-design.md:834) calls for byte comparison and named exceptions. Add frozen pre-C.0 Legacy transcripts for the representative scripts, or identify existing independent golden assertions that cover their bytes and order.

## Coverage and implementation checks

- **Incorporation:** Checked both round 2 findings against revision 3’s task text.
- **Architecture and safety:** Checked gate ownership, disconnect/reset handoff, ready-ring blocking, completion, and the forced-query publication path. The demonstrated ordering gap is B-1.
- **Spec and verification:** Checked §§7.1–7.5 and 8.2–8.4. The plan assigns core tests and inherits implementation gates; compilation, test execution, formatting, Clippy, and portability remain implementation checks. No builds or tests were run.
- **Reading:** 22 spec/source excerpts; two exceeded the 120-line excerpt cap and were counted as two budget units each (**24/24**). Investigation stopped there. Exact opcode classification, every exit path, hardware fixture behavior, and the inherited gate commands remain unassessed; they are not findings of soundness.