# Stage 3b-ii plan — codex review, round 6

**Target:** plan revision 6, prior review round 5.

**Result:** 1 blocking, 0 major, 0 minor. Trend: r1 2B 1M, r2 1B 1M, r3 1B 1M,
r4 0B 1M, r5 1B, r6 1B.

**Reviewer:** `codex exec --sandbox read-only`, single pass
**Instrument:** `docs/superpowers/review/` @ `0245f96b`;
model `gpt-6-sol`; reasoning effort `xhigh`; `codex-cli 0.155.1`.
Counts are comparable only to other reviews citing this same instrument SHA.

**Author verification (2026-09-24):** B-1 CONFIRMED — the forced reprobe runs
synchronously on the core thread (existing behaviour, Legacy and Owner).
Design revision 12 counts it in `L`, services deadlines first after it, and
carries its move off the core thread to 3c (`AdministrativeReprobe`); plan
revision 7 tests a stalled reprobe.

## Verdict

**1 blocking, 0 major, 0 minor**  
Coverage: **COMPLETE FOR DECLARED SCOPE**

This is a design review. It does not establish that the implementation compiles, passes tests, or is approved.

## Incorporation audit

| Prior finding | Status | Assessment |
| --- | --- | --- |
| Round 3 B-1 — forced reprobe bypasses ordering | **APPLIED** | The plan queues `GetScreenResources` behind a dispatched Owner modeset and preserves immediate execution behind a Legacy PRIME probe ([plan:105](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-24-phase-c0-stage-3b-ii-plan-randr-protocol.md:105)). |
| Round 3 M-1 — no independent Legacy oracle | **APPLIED** | Task 5 requires unchanged existing byte assertions and the stage 5 comparison with the external golden ([plan:238](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-24-phase-c0-stage-3b-ii-plan-randr-protocol.md:238)). Their exact coverage remains unassessed. |
| Round 4 M-1 — `KillClient` loses an in-flight publication | **APPLIED** | Inline removal must use disconnect abandonment, with a dispatched-requester test ([plan:143](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-24-phase-c0-stage-3b-ii-plan-randr-protocol.md:143), [plan:169](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-24-phase-c0-stage-3b-ii-plan-randr-protocol.md:169)). |
| Round 5 B-1 — forced query changes Legacy behavior and has no bound | **PARTIAL** | Revision 6 fixes the Legacy behavior, and spec revision 11 permits the Owner wait ([spec:598](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-24-phase-c0-stage-3b-modeset-and-randr-design.md:598)). The claimed bound still lacks a limit for the synchronous reprobe. See B-1. |

## Findings

### Blocking

**B-1 — A synchronous forced reprobe can defeat the Owner-only request bound.** The plan makes `GetScreenResources` an unexpired synchronous FIFO member behind a dispatched Owner modeset ([plan:105](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-24-phase-c0-stage-3b-ii-plan-randr-protocol.md:105), [plan:183](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-24-phase-c0-stage-3b-ii-plan-randr-protocol.md:183)). Once admitted, its handler calls the backend reprobe on the core thread; the backend probes each DRM device synchronously ([source:2952](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver-core/src/core_loop/process_request.rs:2952), [source:24457](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/backend.rs:24457), [source:4871](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/platform.rs:4871)).

For example, Owner modeset A is pending; B’s `GetScreenResources` and C’s `SetCrtcConfig` queue behind it. A completes, then B’s device probe stalls. The core cannot service C’s `Q` timer or answer B until that call returns. Neither the plan nor the spec assigns the reprobe a deadline, while both claim an Owner-only `Q + E` bound; the spec adds `L` only for synchronous **Legacy** executions ([plan:42](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-24-phase-c0-stage-3b-ii-plan-randr-protocol.md:42), [spec:699](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-24-phase-c0-stage-3b-modeset-and-randr-design.md:699), [spec:707](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-24-phase-c0-stage-3b-modeset-and-randr-design.md:707)). This also defeats the umbrella’s whole-request bounded-wait obligation ([umbrella:298](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-22-phase-c0-stage-3-lifecycle-design.md:298)).

Before execution, give synchronous gate members a finite, enforceable execution bound and account for all such work ahead of a waiter, including its timeout and late-publication behavior. Add a test in which the forced reprobe stalls while another waiter’s `Q` expires. The current tests cover a slow synchronous mutation only by servicing deadlines *after* it returns ([plan:193](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-24-phase-c0-stage-3b-ii-plan-randr-protocol.md:193)).

### Major

None.

### Minor

None.

## Coverage and implementation checks

- **Incorporation:** Checked all findings recorded or carried in round 5 against the revised task text.
- **Architecture and safety:** Checked gate intake and ready-ring dispatch, completion wake, client removal, reset handoff, and reprobe execution. B-1 is the confirmed failure-semantic gap.
- **Spec and evidence:** Checked the umbrella obligations and protocol-order gate, plus spec §§7.1–7.5 and 8.2–8.4. The stalled-reprobe case is missing from the proposed evidence.
- **Reading:** 23/24 spec/source excerpts charged; one 124-line source read exceeded the 120-line cap and was counted as two. Exact opcode classification, the existing byte-test inventory, and hardware behavior remain unassessed, not judged sound. Builds, tests, nightly formatting, CI-equivalent Clippy, and portability checks belong to implementation.