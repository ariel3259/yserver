## Verdict

**0 blocking, 1 major, 0 minor**  
Coverage: **INCOMPLETE**

This is a design review, not a claim that the plan compiles, passes tests, or is approved for implementation.

## Incorporation audit

| Round 3 finding | Status | Assessment |
| --- | --- | --- |
| B-1, forced resource query bypasses the gate | **NOT APPLIED** | The plan still treats queries as ungated and supplies only a test requester-less producer ([plan:43](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-24-phase-c0-stage-3b-ii-plan-randr-protocol.md:43), [plan:157](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-24-phase-c0-stage-3b-ii-plan-randr-protocol.md:157)). The current forced query still publishes directly ([process_request.rs:2952](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver-core/src/core_loop/process_request.rs:2952), [backend.rs:24457](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/backend.rs:24457)). Converting reprobe into an executed `AdministrativeReprobe` transition belongs to the explicitly excluded 3c scope, so I do not repeat this as a 3b-ii finding. |
| M-1, no pre-C.0 Legacy oracle in the wire differential | **NOT APPLIED** | Task 5 still compares Legacy and Owner fixtures using the modified core ([plan:181](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-24-phase-c0-stage-3b-ii-plan-randr-protocol.md:181)). The umbrella separates that layer-1 differential from the already captured, external Legacy golden ([umbrella:376](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-22-phase-c0-stage-3-lifecycle-design.md:376), [umbrella:396](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-22-phase-c0-stage-3-lifecycle-design.md:396)). Its absence from this plan or repository is not a defect. |

## Findings

### Blocking

None.

### Major

**M-1 — `KillClient` cleanup does not prove that an in-flight publication survives.** Task 2 permits the inline `KillClient` path to “prune” an in-flight requester, while its named `KillClient` test kills only a *waiting* head ([plan:107](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-24-phase-c0-stage-3b-ii-plan-randr-protocol.md:107), [plan:131](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-24-phase-c0-stage-3b-ii-plan-randr-protocol.md:131)). `KillClient` can disconnect another client inline ([process_request.rs:22849](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver-core/src/core_loop/process_request.rs:22849)). If D kills A after A’s Owner modeset is dispatched, cleanup that removes A’s whole in-flight entry loses the installed change’s publication; the ordinary disconnect test would still pass. That violates [spec §7.2](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-24-phase-c0-stage-3b-modeset-and-randr-design.md:629). Require this path to apply the same abandonment result as ordinary disconnect: remove A’s reply attachment, but retain the gate’s publication on `ContinuesWithoutRequester`. Add a test that kills the dispatched requester and checks the listener’s notifications and subsequent waiter admission.

### Minor

None.

## Coverage and implementation checks

- **Incorporation:** Audited both prior findings against the task text; neither is claimed fixed by a revision-4 correction.
- **Architecture and safety:** Checked gate ownership, request queuing, completion, client removal, reset handoff, and requester-less wake delivery against targeted baseline code. M-1 is the remaining in-scope handoff gap.
- **Spec and verification:** Checked the six obligations, spec §§7.1–7.5 and 8.2, and the umbrella protocol-order gate. The plan assigns implementation build and test gates; none were run.
- **Reading limit:** 21 spec/source reads, counted as **24/24 windows** because three source reads exceeded the requested 120-line size. I stopped investigating there. Exact opcode classification, every failure exit, and the real 3c reprobe handoff remain unassessed; they are not findings of soundness. Rust compilation, tests, formatting, Clippy, and portability remain implementation checks.