# Stage 3b-ii plan — codex review, round 7

**Target:** plan revision 7, prior review round 6.

**Result:** 0 blocking, 1 major, 1 minor. Trend: r1 2B 1M, r2 1B 1M, r3 1B 1M,
r4 0B 1M, r5 1B, r6 1B, r7 0B 1M 1m.

**Reviewer:** `codex exec --sandbox read-only`, single pass
**Instrument:** `docs/superpowers/review/` @ `0245f96b`;
model `gpt-6-sol`; reasoning effort `xhigh`; `codex-cli 0.155.1`.
Counts are comparable only to other reviews citing this same instrument SHA.

**Author verification (2026-09-24):** M-1 CONFIRMED (the XDMCP branch cancels and
returns, `run.rs:1819`); m-1 CONFIRMED. Both applied in plan revision 8. Loop
closed: no blocking finding.

## Verdict

**0 blocking, 1 major, 1 minor**  
Coverage: **COMPLETE FOR DECLARED SCOPE**

This is a design review; it does not establish that the implementation compiles, passes tests, or is approved.

## Incorporation audit

| Prior finding | Status | Assessment |
| --- | --- | --- |
| Round 3 B-1 — forced reprobe bypasses ordering | **APPLIED** | Task 1 queues `GetScreenResources` behind a dispatched Owner modeset and preserves immediate execution behind a Legacy PRIME probe ([plan:110](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-24-phase-c0-stage-3b-ii-plan-randr-protocol.md:110)). |
| Round 3 M-1 — no independent Legacy oracle | **APPLIED** | Task 5 requires unchanged existing RANDR byte assertions and the stage 5 golden comparison ([plan:244](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-24-phase-c0-stage-3b-ii-plan-randr-protocol.md:244)). The assertions’ exact coverage remains unassessed. |
| Round 4 M-1 — `KillClient` loses an in-flight publication | **APPLIED** | Task 2 applies disconnect abandonment to inline removal and names a dispatched-requester test ([plan:147](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-24-phase-c0-stage-3b-ii-plan-randr-protocol.md:147), [plan:174](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-24-phase-c0-stage-3b-ii-plan-randr-protocol.md:174)). |
| Round 5 B-1 — forced query changes Legacy behavior and has no bound | **PARTIAL** | The Legacy exception is preserved, and revision 12 accounts for synchronous reprobe time. The plan’s goal still states the old Owner-only bound; see m-1. |
| Round 6 B-1 — stalled reprobe defeats the claimed bound | **PARTIAL** | Task 3 now services deadlines after reprobe and tests that ordering ([plan:181](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-24-phase-c0-stage-3b-ii-plan-randr-protocol.md:181), [plan:198](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-24-phase-c0-stage-3b-ii-plan-randr-protocol.md:198)). Spec revision 12 assigns reprobe time to `L_reprobe` and carries its move off the core thread to 3c ([spec:721](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-24-phase-c0-stage-3b-modeset-and-randr-design.md:721)). The plan has not updated its stated bound. |

## Findings

### Blocking

None.

### Major

**M-1 — The XDMCP termination path bypasses the terminal-result handoff.** Task 2 defers a generation reset or `-terminate` until an install-capable mutation reaches a terminal result ([plan:155](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-24-phase-c0-stage-3b-ii-plan-randr-protocol.md:155); [spec:654](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-24-phase-c0-stage-3b-modeset-and-randr-design.md:654)). The loop also has an orderly XDMCP termination branch that cancels pending backend requests and returns directly ([source:1819](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver-core/src/core_loop/run.rs:1819)); XDMCP `-once` can produce that outcome ([source:500](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver-core/src/core_loop/xdmcp.rs:500)). If the session client leaves while another client’s Owner modeset is dispatched, this branch can exit before the install result is known. Specify that XDMCP termination uses the same terminal-result handoff before cancellation and exit, and test that sequence.

### Minor

**m-1 — The goal still promises the obsolete Owner-only bound.** The goal says every parked request is answered within `Q + E`, adding `L` only in a mixed server ([plan:47](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-24-phase-c0-stage-3b-ii-plan-randr-protocol.md:47)). Revision 12 instead states `Q + E + L_reprobe` even on an Owner-only server ([spec:721](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-24-phase-c0-stage-3b-modeset-and-randr-design.md:721)). An Owner modeset followed by a stalled forced reprobe is the concrete counterexample to the goal’s formula. Update the goal and Task 3’s bound statement to include `L_reprobe`.

## Coverage and implementation checks

- **Incorporation:** Checked every finding listed or carried in round 6 against revision 7.
- **Architecture and safety:** Checked gate admission, ready-ring dispatch, completion wake, client cleanup, reset, reprobe, and the XDMCP exit path.
- **Spec and evidence:** Checked the relevant umbrella obligations and protocol-order gate, plus spec §§7.1–7.5, 8.2, and 8.4. The named stalled-reprobe test establishes deadline ordering after return, not a fixed duration for the reprobe.
- **Reading:** **24/24 charged excerpts** beyond the plan and prior review; 23 reads, with one 123-line source slice charged as two. Exact opcode classification, existing byte-test coverage, inherited implementation gates, and hardware behavior remain unassessed. Builds, real tests, nightly formatting, CI-equivalent Clippy, and portability checks belong to implementation.