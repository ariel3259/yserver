# Stage 3c-i plan — codex review, round 4

**Target:** revision 4 (`0f09a5a9`), fourth review (prior: round 3).

**Result:** 1 blocking, 2 major, 0 minor; coverage COMPLETE FOR DECLARED SCOPE (24/24). Round 3: both applied.

**Reviewer:** `codex exec --sandbox read-only`, single pass
**Instrument:** `docs/superpowers/review/` @ `067a1c12`;
model `gpt-6-sol`; reasoning effort `xhigh`; `codex-cli 0.155.1`.
Counts are comparable only to other reviews citing this same instrument SHA.

**Author verification (2026-09-26):** all three CONFIRMED.

- **B-1** — `core_loop/run.rs` `drain_ready_crtc_configs_with_gate_policy`: a token without a gate publication is cancelled and no reply is sent. Rev 5 resolves the token `Failed` through the ordinary ready path (reply `Failed`, nothing published — the existing 3b-ii rule) instead of removing the publication; a new yserver-core test with a parked request.
- **M-1** — `drain_requesterless_publications` queues publications whose `update_state` closure may capture an old projection. Rev 5: the core filters every publication against the withdrawn ids; test with a publication queued before the withdrawal.
- **M-2** — `c0_3bi_core_driver_until_mode` builds `ServerState::new()`. Rev 5: a state-carrying variant of the same driver for every byte-asserting backend test.

## Review as received

## Verdict

**1 blocking, 2 major, 0 minor**  
**Coverage: COMPLETE FOR DECLARED SCOPE**

This is a design review. It does not establish that the plan compiles, passes tests, or is approved for execution.

## Incorporation audit

| Round-3 finding | Status | Revision 4 |
| --- | --- | --- |
| B-1 — interim acquire can write to Owner through Legacy resume | **APPLIED** | Task 2 scopes resume to Legacy devices and keeps every Owner device closed until Task 4; its interim hardware assertion no longer expects an Owner frame ([plan:189](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-26-phase-c0-stage-3c-i-plan-vt.md:189), [plan:213](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-26-phase-c0-stage-3c-i-plan-vt.md:213)). |
| B-2 — acquire episode starts too late | **APPLIED** | Task 4 begins it before probe and Legacy resume, stages Legacy changes, and requires the core to reserve the turn before draining requests ([plan:270](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-26-phase-c0-stage-3c-i-plan-vt.md:270)). |

## Findings

### Blocking

**B-1 — Deadline cancellation has no contract for delivering the client’s failed reply.**  
Task 2 requires a dispatched client modeset’s token to resolve `Failed` *and* its gate publication to be withdrawn at the deadline ([plan:170](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-26-phase-c0-stage-3c-i-plan-vt.md:170)); the spec requires unknown work to close without a late state change ([spec:156](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-26-phase-c0-stage-3c-vt-and-hotplug-design.md:156)). Currently, the core owns the parked client reply. If the gate publication is absent when a token becomes ready, it takes that reply, cancels the token, and continues without replying ([run.rs:1627](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver-core/src/core_loop/run.rs:1627)); the reply path requires a retained publication ([run.rs:1671](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver-core/src/core_loop/run.rs:1671)). Thus a modeset that never answers can pass the hand-off deadline while its requester remains unanswered. Specify one core-visible terminalization operation that sends `Failed`, releases the gate, suppresses state publication, and rejects late results. Exercise it with an actual parked core request.

### Major

**M-1 — Already queued publications need a post-withdrawal rule.**  
Task 1 withdraws from the backend model and published projection, but says only that other publications are built from the backend model ([plan:102](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-26-phase-c0-stage-3c-i-plan-vt.md:102)). The core can already hold a requester-less publication behind a gate ([run.rs:1726](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver-core/src/core_loop/run.rs:1726)); that publication contains a deferred state-update closure ([trait_def.rs:75](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver-core/src/backend/trait_def.rs:75)). If it captured a full projection before an urgent withdrawal, running it afterward can restore the withdrawn output, contrary to the spec’s rule for **every** later publication ([spec:327](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-26-phase-c0-stage-3c-vt-and-hotplug-design.md:327)). Require queued publications to be rebuilt, invalidated, or filtered against withdrawals at publication time; test a publication queued *before* the withdrawal.

**M-2 — The backend driver cannot by itself prove the named client-visible outcomes.**  
The plan assigns backend scenarios to the existing core-entry driver, including the deadline `Failed` reply and exact VT client bytes ([plan:122](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-26-phase-c0-stage-3c-i-plan-vt.md:122), [plan:207](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-26-phase-c0-stage-3c-i-plan-vt.md:207), [plan:302](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-26-phase-c0-stage-3c-i-plan-vt.md:302)). That driver creates a fresh `ServerState` for its progress loop ([backend.rs:65973](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/backend.rs:65973)); it does not retain the entry’s clients and parked core request. A test can therefore establish backend progress while missing a lost reply or wrong client notification. Use the existing core-loop harness, or extend the existing driver to carry the same state and pending gate through entry, completion, and byte assertions. The spec requires production-entry scenarios and an explicit end-state check ([spec:467](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-26-phase-c0-stage-3c-vt-and-hotplug-design.md:467)).

### Minor

None.

## Coverage and implementation checks

**24/24 bounded spec and source excerpts used.** The incorporation audit checked both round-3 findings. Architecture checks covered VT entry, lifecycle action delivery, core gate ordering, and the Task 2–4 boundary. Safety checks covered deadline closure, late results, and publication ordering. Spec and verification checks covered §3, relevant §4.1 and §5 rules, named VT evidence, and per-task gates.

The plan assigns formatting, all-targets clippy, tests, and hardware runs to implementation ([plan:329](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-26-phase-c0-stage-3c-i-plan-vt.md:329), [plan:333](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-26-phase-c0-stage-3c-i-plan-vt.md:333)). No build or test was run. Console seam fidelity, exact fd-alias retirement, and real kernel behavior were not verified; this review makes no soundness claim for them.