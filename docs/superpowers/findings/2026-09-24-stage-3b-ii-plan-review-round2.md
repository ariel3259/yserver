# Stage 3b-ii plan — codex review, round 2

**Target:** `docs/superpowers/plans/2026-09-24-phase-c0-stage-3b-ii-plan-randr-protocol.md`
revision 2, prior review round 1.

**Result:** 1 blocking, 1 major, 0 minor; coverage COMPLETE FOR DECLARED SCOPE.
Trend: r1 2B 1M, r2 1B 1M.

**Reviewer:** `codex exec --sandbox read-only`, single pass
**Instrument:** `docs/superpowers/review/` @ `0245f96b`;
model `gpt-6-sol`; reasoning effort `xhigh`; `codex-cli 0.155.1`.
Counts are comparable only to other reviews citing this same instrument SHA.

**Author verification (2026-09-24):** B-1 CONFIRMED — the reset cancels parked
tokens and seeds RANDR state from a backend snapshot (`reset.rs`); a dispatched
Owner modeset could install after it. Plan revision 3 and design revision 10
defer the reset until the install-capable mutation is terminal (bounded by
`E`). M-1 CONFIRMED — a parked request blocks its own client
(`run.rs:699`); Task 5 now uses three requester connections.

## Verdict

**1 blocking, 1 major, 0 minor**  
Coverage: COMPLETE FOR DECLARED SCOPE

This is a design-review result; it does not establish that the plan compiles, passes tests, or is approved for implementation.

## Incorporation audit

| Prior finding | Status | Assessment |
| --- | --- | --- |
| B-1, requester-less wake | **APPLIED** | Task 4 now requires a wake on enqueue and a drain even when no CRTC token is ready ([plan:145](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-24-phase-c0-stage-3b-ii-plan-randr-protocol.md:145)). |
| B-2, inline client removal | **PARTIAL** | Task 2 names `KillClient`, requires pruning, and adds its test ([plan:100](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-24-phase-c0-stage-3b-ii-plan-randr-protocol.md:100), [plan:117](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-24-phase-c0-stage-3b-ii-plan-randr-protocol.md:117)). Its broader claim that *every* removal path is covered misses generation reset; see B-1 below. |
| M-1, compound wire scripts | **PARTIAL** | Task 5 adds the requested scripts ([plan:175](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-24-phase-c0-stage-3b-ii-plan-randr-protocol.md:175)), but its specified connection count cannot drive the three-request mixed-server script; see M-1 below. |

## Findings

### Blocking

**B-1 — Generation reset can discard an install-capable publication.** Task 2 says a dispatched Owner modeset survives requester departure until its result is published ([plan:92](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-24-phase-c0-stage-3b-ii-plan-randr-protocol.md:92)); the spec requires that continuation ([spec §7.2](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-24-phase-c0-stage-3b-modeset-and-randr-design.md:628)). If its requester is the last client, the loop can enter generation reset ([run.rs:1836](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver-core/src/core_loop/run.rs:1836)). Reset currently cancels all parked CRTC tokens, clears per-client queues, and seeds new RANDR state from a backend snapshot ([reset.rs:338](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver-core/src/core_loop/reset.rs:338), [reset.rs:400](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver-core/src/core_loop/reset.rs:400)). A dispatched commit can then install after that snapshot, with its publication lost or applied to the wrong generation. Define the reset handoff for an install-capable token: retain it to a terminal result and seed or reconcile the new generation from that result, without carrying an old-client reply or event into the new session.

### Major

**M-1 — The wire differential has too few requester connections for its mixed-server case.** Task 5 fixes the fixture at two connections, “requester and listener,” yet requires Owner A in flight with Legacy B and Owner C queued ([plan:167](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-24-phase-c0-stage-3b-ii-plan-randr-protocol.md:167), [plan:180](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-24-phase-c0-stage-3b-ii-plan-randr-protocol.md:180)); the spec requires that wire-level sequence ([spec §8.2](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-24-phase-c0-stage-3b-modeset-and-randr-design.md:825)). A pending request blocks later requests from its own client ([run.rs:699](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver-core/src/core_loop/run.rs:699), [spec §7.1](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-24-phase-c0-stage-3b-modeset-and-randr-design.md:597)). Sharing a requester therefore prevents all three requests from reaching the gate in the required order, so the proposed test cannot prove C’s deadline behavior. Specify three independent requester connections for this script, with a listening connection and per-connection byte assertions.

## Coverage and implementation checks

- **Incorporation:** Checked all three prior findings against revision 2’s task text.
- **Architecture and safety:** Checked gate scheduling, disconnect, completion, timeout wake, and reset handoffs against targeted source.
- **Spec and evidence:** Checked §§7–8.2 and the umbrella protocol gate. The inherited implementation gates include nightly format, CI-form Clippy, crate tests, and every integration test file ([3b-i-1 plan:95](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-24-phase-c0-stage-3b-i-1-plan-modeset-execution.md:95)).
- **Reading:** 24/24 bounded spec/source excerpts. Exact opcode classification, Owner fixture behavior, requester-less producer internals, portability, and whether tests or code compile remain unassessed or assigned to implementation. No builds or tests were run.