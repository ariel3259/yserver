# Stage 3b-ii plan — codex review, round 5

**Target:** plan revision 5 (revisions 4 and 5 together), prior review round 4.

**Result:** 1 blocking, 0 major, 0 minor. Trend: r1 2B 1M, r2 1B 1M, r3 1B 1M,
r4 0B 1M (on revision 3), r5 1B.

**Reviewer:** `codex exec --sandbox read-only`, single pass
**Instrument:** `docs/superpowers/review/` @ `0245f96b`;
model `gpt-6-sol`; reasoning effort `xhigh`; `codex-cli 0.155.1`.
Counts are comparable only to other reviews citing this same instrument SHA.

**Author verification (2026-09-24):** B-1 CONFIRMED — revision 4 contradicted the
design's query rule and changed pure-Legacy behaviour. Resolved in design
revision 11 (the forced reprobe joins the gate only behind an install-capable
mutation; named exception 6) and plan revision 6. Listed for the user.

## Verdict

**1 blocking, 0 major, 0 minor**  
Coverage: **COMPLETE FOR DECLARED SCOPE**

This is a design review; it does not establish that implementation compiles, passes tests, or is approved.

## Incorporation audit

| Prior finding | Status | Assessment |
| --- | --- | --- |
| Round 3 B-1 — forced resource query bypasses ordering | **TRADED** | [Task 1](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-24-phase-c0-stage-3b-ii-plan-randr-protocol.md:97) now orders the reprobe through the gate, but makes a query wait, contrary to the authoritative spec. See B-1. |
| Round 3 M-1 — no independent Legacy oracle | **APPLIED** | [Task 5](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-24-phase-c0-stage-3b-ii-plan-randr-protocol.md:227) now requires unchanged existing byte assertions and the stage 5 comparison with the external golden. The implementer must still list the tests; their exact coverage was not assessed here. The golden’s absence from the repository is not a defect. |
| Round 4 M-1 — `KillClient` can lose an in-flight publication | **APPLIED** | [Task 2](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-24-phase-c0-stage-3b-ii-plan-randr-protocol.md:132) applies disconnect abandonment to inline removal and names a dispatched-requester `KillClient` test. |

## Findings

### Blocking

**B-1 — The forced resource query now waits despite the spec’s query contract.** The authoritative spec says queries, including `GetScreenResources`, **never wait** and read published state ([§7.1](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-24-phase-c0-stage-3b-modeset-and-randr-design.md:589)). The plan instead makes `GetScreenResources` a synchronous FIFO member that waits behind an in-flight mutation ([Task 1](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-24-phase-c0-stage-3b-ii-plan-randr-protocol.md:97)), while its own blocking decision still says queries never block ([plan](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-24-phase-c0-stage-3b-ii-plan-randr-protocol.md:58)).

For example, a Legacy client parks in a PRIME probe; another client sends `GetScreenResources`. Today that handler reprobes before replying ([source](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver-core/src/core_loop/process_request.rs:2939)); under Task 1, its reply waits for the first client’s terminal result. This changes pure-Legacy query behavior beyond the spec’s sole named concurrent-PRIME exception ([§7.1](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-24-phase-c0-stage-3b-modeset-and-randr-design.md:619), [§8.4](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-24-phase-c0-stage-3b-modeset-and-randr-design.md:909)). The plan also gives this newly waiting query no explicit deadline.

The reprobe genuinely can publish, so its ordering needs a contract: KMS currently rebuilds state and emits notifications during it ([source](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/backend.rs:10348)). Before execution, either define a sensing/publication split that preserves the query rule, or amend the authoritative spec to name and bound the forced-query exception. Align the plan’s blocking decision with that choice.

### Major

None.

### Minor

None.

## Coverage and implementation checks

- **Incorporation:** Checked both round 3 findings and the round 4 finding against revisions 4 and 5.
- **Architecture and safety:** Checked the ready ring, completion, client removal, reset handoff, and reprobe publication paths with targeted source reads. The plan states tests for `KillClient`, reset, queue expiry, and requester-less wake ordering; those tests were not run.
- **Spec and evidence:** Checked the six umbrella obligations, spec §§7.1–7.5 and 8.2–8.4, and the protocol-order gate. B-1 is a spec conflict, not a predicted compiler failure.
- **Reading:** 20 spec/source excerpts, charged as **21/24** because one accidental 131-line excerpt exceeded the 120-line cap and was counted twice. Exact opcode classification, every backend failure exit, live hardware fixture behavior, and the existing byte-test inventory remain unassessed—not findings of soundness. Builds, tests, nightly formatting, CI-equivalent Clippy, and portability checks belong to implementation.