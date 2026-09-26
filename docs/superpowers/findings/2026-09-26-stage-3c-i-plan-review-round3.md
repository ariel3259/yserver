# Stage 3c-i plan — codex review, round 3

**Target:** revision 3 (`02f16585`), third review (prior: round 2).

**Result:** 2 blocking, 0 major, 0 minor; coverage COMPLETE FOR DECLARED SCOPE (24/24). Round 2: B-2, M-3 applied; M-1 partial; B-1, M-2 traded.

**Reviewer:** `codex exec --sandbox read-only`, single pass
**Instrument:** `docs/superpowers/review/` @ `067a1c12`;
model `gpt-6-sol`; reasoning effort `xhigh`; `codex-cli 0.155.1`.
Counts are comparable only to other reviews citing this same instrument SHA.

**Author verification (2026-09-26):** both CONFIRMED.

- **B-1** — `on_vt_acquire` → `drive_vt_event` → `run_resume`, which relights through the platform; rev 3's interim acquire kept that for healthy Owner devices. Rev 4: Owner devices stay closed until Task 4; Legacy devices resume scoped.
- **B-2** — `run_resume` publishes via `fire_randr_changes` during the snapshot apply, and the core drains pending requests right after `VtAcquire`. Rev 4: the episode begins first, the core reserves the turn before draining, the Legacy change is staged for `EpisodeEnd`.

## Review as received

## Verdict

**2 blocking, 0 major, 0 minor**  
**Coverage: COMPLETE FOR DECLARED SCOPE**

This is a design-review result; it does not establish that the plan compiles, passes tests, or is approved for execution.

## Incorporation audit

| Round-2 finding | Status | Revision 3 |
| --- | --- | --- |
| B-1 — unsafe intermediate release route | **TRADED** | Task 2 now delivers the whole release route, but its interim acquire leaves an unsafe Owner path (B-1 below). |
| B-2 — occupied client slot at deadline | **APPLIED** | Task 2 covers any nonterminal slot occupant, resolves a client token `Failed`, prevents late publication, and names a production-entry test ([plan:163](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-26-phase-c0-stage-3c-i-plan-vt.md:163)). |
| M-1 — no core episode turn | **PARTIAL** | Task 1 adds begin/end signals and a gate test, but Task 4 starts the episode too late and does not establish when the core consumes `EpisodeBegin` (B-2 below). |
| M-2 — acquire precedes reinstall | **TRADED** | Task 4 combines acquire and reinstall; Task 2 still exposes an interim Owner acquire. |
| M-3 — hardware test scheduled before written | **APPLIED** | Task 2 writes the release test before its first scheduled run; Task 4 extends it for acquire ([plan:201](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-26-phase-c0-stage-3c-i-plan-vt.md:201), [plan:282](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-26-phase-c0-stage-3c-i-plan-vt.md:282)). |

## Findings

### Blocking

**B-1 — Task 2’s interim acquire can write to an Owner device through Legacy resume.**  
Task 2 switches on the Owner release route but says every healthy device keeps today’s acquire resume until Task 4; its hardware test expects the Owner to compose after that interim return ([plan:182](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-26-phase-c0-stage-3c-i-plan-vt.md:182), [plan:201](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-26-phase-c0-stage-3c-i-plan-vt.md:201)). Today `on_vt_acquire` calls `drive_vt_event`, which calls `run_resume`; that path directly relights outputs through the platform ([backend.rs:25463](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/backend.rs:25463), [backend.rs:16039](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/backend.rs:16039), [backend.rs:15039](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/backend.rs:15039)). A switch away and back after Task 2 can therefore issue a Legacy KMS write on an Owner device, outside its executor and without the required from-scratch lifecycle reinstall ([spec:230](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-26-phase-c0-stage-3c-vt-and-hotplug-design.md:230)). Give Task 2 an Owner-safe interim acquire that keeps Owner scanout closed until Task 4, and limit that task’s hardware assertion to release; alternatively, deliver Owner acquire with Task 2.

**B-2 — The acquire episode does not own the turn before mixed-server work and request dispatch.**  
Task 4 runs scoped Legacy resume before creating `AcquireEpisode`; it signals `EpisodeBegin` only before the first Owner reinstall ([plan:231](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-26-phase-c0-stage-3c-i-plan-vt.md:231), [plan:257](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-26-phase-c0-stage-3c-i-plan-vt.md:257)). The current Legacy resume emits RANDR changes while applying its snapshot ([backend.rs:15021](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/backend.rs:15021)). On a mixed server whose topology changed while away, those changes can reach clients before the Owner reinstall is terminal, contrary to the spec’s single turn and terminal-participant publication rule ([spec:305](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-26-phase-c0-stage-3c-vt-and-hotplug-design.md:305)). There is a second ordering gap: the core handles `VtAcquire` and then drains pending requests ([run.rs:2302](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver-core/src/core_loop/run.rs:2302), [run.rs:2323](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver-core/src/core_loop/run.rs:2323)); the plan does not require it to consume the queued begin signal before admitting one. Start the episode before scoped Legacy resume, stage its notifications for `EpisodeEnd`, and require the core to reserve the turn after the VT entry and before draining pending requests. Test that ordering through the production acquire entry.

### Major

None.

### Minor

None.

## Coverage and implementation checks

**24/24 bounded spec and source excerpts used.** The incorporation audit covered all five round-2 findings. Architecture checks covered VT entry, lifecycle dispatch, the core gate, episode delivery, and mixed-server ordering. Safety checks covered occupied-slot deadline closure and safe task boundaries. Spec and evidence checks covered §3, the relevant §4.1 publication rules, §5 exceptions, and §6’s named tests and hardware schedule.

The plan assigns formatting, regular all-targets clippy, tests, and hardware runs to implementation ([plan:307](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-26-phase-c0-stage-3c-i-plan-vt.md:307), [plan:311](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-26-phase-c0-stage-3c-i-plan-vt.md:311)). No build or test was run. Exact fd-alias retirement, console seam behavior, and fixture fidelity were not inspected; this review makes no soundness claim for them.