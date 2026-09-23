# Plan Cp (copied scanout route) — codex review, round 2

**Target:** `docs/superpowers/plans/2026-09-22-phase-c0-stage-2c-iii-plan-cp-copied-route.md`
revision 2 (`21edf195`), against the copied-route design at revision 4 (`80089003`).
**Prior:** round 1 (`../findings/2026-09-22-stage-2c-iii-plan-cp-review-round1.md`).

**Reviewer:** `codex exec --sandbox read-only`, single pass
**Instrument:** `docs/superpowers/review/` @ `da807b70`;
model `gpt-5.6-sol`; reasoning effort `xhigh`; `codex-cli 0.155.1`.
Counts are comparable only to other reviews citing this same instrument SHA.
Coverage COMPLETE FOR DECLARED SCOPE, **16/24 excerpts** — the first round in
this document's lineage that did not exhaust its reading budget. Sink-ticket
facilities, copied-fixture construction, the §9.5 enumeration, the full ledger
gates and hardware reachability are recorded as unassessed.

**Author verification (2026-09-22), every finding checked against the tree:**

- **B-1 — CONFIRMED, and it reaches into the spec, not just the plan.** The
  `OutputScanout::Copied` arm calls `submit_copied_scanout_render`
  (`scene.rs:6532`; the function at `scene.rs:10041`), which takes a raw
  `vk::Fence` and **no `ResourceService`** and returns no managed batch, while
  the `Shared` arm directly above passes the service and receives one
  (`scene.rs:6521`). The design's §3.2 had described the compose stage from the
  shared arm and assumed the copied arm matched — the same class of unchecked
  premise its own round 2 caught. So stage A's source was unowned by the service
  for the whole of A's write, and CP-4c's paired phase excludes tick selection
  only. Fixed in **spec revision 5** (CP-4 now converts stage A) and in the plan
  (Task 2, Q39/Q40).
- **M-1 and M-2 — CONFIRMED, and they are one finding.** Both name the same
  defect: the plan delegated a real design choice to the implementer and called
  it a report, which contradicts the plan's own F8 rule. Decided here instead,
  from the tree: `FencePool` is built on one `Arc<VkContext>` and the platform's
  is the main device's (`platform.rs:2870`), which cannot signal a submission on
  `sink_vk` (`vk/scanout.rs:949`), so the copied pool owns a `FencePool` over
  the sink context and the ticket recycles at the existing point. The wake
  reuses the existing container rather than a sibling, because both cancellation
  operations already clear it and the loop already drains it at three sites — a
  sibling would have to re-earn six connections — with the entry gaining a stage
  discriminator and correlation by `(output_key, bo_idx, job_id)` plus stage.

**Assessment.** Round 1's three findings are audited as applied and none
regressed. The blocking finding is the third time in this document's lineage
that a premise generalised from a neighbouring code path turned out not to hold
for the copied one; the method rule the plan already carries now has a second
half: **check the copied arm specifically, never the shared arm as its proxy.**

Revision 3 of the plan and revision 5 of the spec incorporate all three.

---

## Verdict

**1 blocking, 2 major, 0 minor**

Coverage: **COMPLETE FOR DECLARED SCOPE**

This is a design-review result only; it does not claim compilation, test success, or implementation approval.

## Incorporation audit

| Prior finding | Status | Assessment |
|---|---|---|
| M-1 — promotion evidence preceded the receipt producer | **APPLIED** | Preparation is now Task 2, including production wake registration and receipt creation; Task 3 explicitly consumes that production path and forbids injected receipts or test seams ([plan:123](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-22-phase-c0-stage-2c-iii-plan-cp-copied-route.md:123), [plan:148](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-22-phase-c0-stage-2c-iii-plan-cp-copied-route.md:148)). |
| M-2 — failed submission could still offer | **APPLIED** | Task 4 now requires `Displaced`, no offer, and service-owned discharge for both `gpu_submitted` outcomes; Q36/Q37 target the omitted behavior ([plan:171](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-22-phase-c0-stage-2c-iii-plan-cp-copied-route.md:171)). |
| M-3 — only per-output cancellation was covered | **APPLIED** | Decision 11 and Task 4 cover both per-output cancellation and whole-queue clearing, with Q38 requiring a VT-suspend stale-completion observation ([plan:36](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-22-phase-c0-stage-2c-iii-plan-cp-copied-route.md:36), [plan:175](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-22-phase-c0-stage-2c-iii-plan-cp-copied-route.md:175)). |

## Findings

### Blocking

#### B-1 — Task 2 never requires A’s source write to become resource-owned before A reaches the GPU

The specification requires A’s batch to own the source write lease and GPU obligation, with preparation completed before any raw handle reaches the GPU ([spec:189](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-22-phase-c0-stage-2c-iii-copied-route-design.md:189), [spec:200](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-22-phase-c0-stage-2c-iii-copied-route-design.md:200)). The existing copied submission performs A’s GPU render without the resource service and returns no managed batch ([scene.rs:6532](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/scene.rs:6532), [scene.rs:10042](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/scene.rs:10042)).

Task 2 begins its stated sequence with “A’s batch registered and serviced,” but specifies only B’s destination/write and source/read preparation and tests ([plan:127](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-22-phase-c0-stage-2c-iii-plan-cp-copied-route.md:127), [plan:134](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-22-phase-c0-stage-2c-iii-plan-cp-copied-route.md:134)). Nothing requires creating A’s source batch before A submission or mutates its omission/delay.

Concrete failure: an implementation synthesizes or registers A’s batch only at completion, then correctly prepares B. Every named Task 2 mutation can pass, while during A execution the source has no service lease or obligation and may be reserved or recycled by another service consumer. The paired destination phase only proves tick-selection exclusion; it is not A’s resource ownership.

Smallest correction: Task 2 must explicitly prepare A’s source write batch before `submit_copied_scanout_render`, retain it through A’s completion, and add evidence/mutation that omitting or delaying A’s source obligation past submission fails.

### Major

#### M-1 — Fence-ticket ownership remains an implementation-time design choice

The spec assigns the plan the decision about the sink-context `FenceTicket` source and whether it needs a pool ([spec:458](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-22-phase-c0-stage-2c-iii-copied-route-design.md:458)). The plan instead tells the implementer to decide and report it before coding ([plan:35](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-22-phase-c0-stage-2c-iii-plan-cp-copied-route.md:35), [plan:129](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-22-phase-c0-stage-2c-iii-plan-cp-copied-route.md:129)), contradicting its own F8 rule for open design choices.

Without a reviewed allocation, exclusivity, and recycle point, a sink fence can be reset or reused while its B batch still treats it as completion authority.

Smallest correction: name the sink ticket owner/source and the exact point at which the ticket becomes reusable.

#### M-2 — The wake container and event-delivery contract are still undecided

The spec permits either the existing poller or a sibling but requires the plan to choose and justify one ([spec:462](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-22-phase-c0-stage-2c-iii-copied-route-design.md:462)). The plan delegates that choice to Task 2 ([plan:131](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-22-phase-c0-stage-2c-iii-plan-cp-copied-route.md:131)).

This matters because the current container is explicitly source-renderer-oriented and carries only job/output/BO identity ([platform.rs:4887](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/platform.rs:4887)). Reuse therefore needs a defined A-versus-B dispatch contract; a sibling needs defined loop draining and both teardown connections. Cancellation constraints alone do not decide delivery ownership.

Smallest correction: select the container and specify its event payload, consumer/stage correlation, loop drain, and both cancellation connections.

### Minor

None.

## Coverage and implementation checks

- **Incorporation:** audited all three prior findings; all were applied.
- **Architecture/contracts:** checked task ordering, A/B batch handoff, receipt consumption, wake delivery, cancellation, and current copied submission.
- **Safety/ownership:** checked lease timing, obligation correlation, failure disposition, supersession, and stale-completion teardown.
- **Compliance/verification:** compared all §8.2 criteria and mutations with the task evidence and confirmed build, clippy, portability, Vulkan, and hardware gates are assigned.

Excerpts used: **16/24** — five spec and eleven source excerpts.

Unassessed and not deemed sound: concrete sink-ticket facilities, copied-fixture construction, the Task 2 §9.5 consumer enumeration, full ledger gates, and hardware reachability. Exact Rust signatures, borrowing, compilation, formatting, clippy, cross-target builds, Vulkan execution, and hardware mutation runs remain deferred to implementation.