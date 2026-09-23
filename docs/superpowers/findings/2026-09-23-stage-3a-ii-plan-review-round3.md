# Stage 3a-ii plan — codex review, round 3

**Target:** revision 4 (`9fb60a44`). **Result:** 0 blocking, 0 major, 0 minor; coverage INCOMPLETE (24/24). Trend: r1 2B 4M, r2 0B 2M, r3 0/0/0. Incorporation: both round-2 findings APPLIED.

**Reviewer:** `codex exec --sandbox read-only`, single pass; instrument `docs/superpowers/review/` @ `0245f96b`; `gpt-6-sol` `xhigh`; `codex-cli 0.155.1`.

**Disposition (2026-09-23):** the round was focused on what round 2 could not assess and established it: Task 3's `ACTIVE`-only shape is expressible through `CommitDescription`/`CrtcPower` with the old-active CRTC in `ExpectedCompletionCrtcs` (`owner/closure.rs:196`, `owner/build.rs:126`), and the bootstrap fix matches C.0 §10.3 (`owner/device.rs:1312`, `owner/deadlines.rs:37`). Still unassessed and **carried into implementation, not waived**: Task 7's per-read Owner reachability classification (the plan already requires a mutation per inventoried read, which is that check), and the compose-timestamp flag's interaction with Tasks 5–6 (to be checked by the coordinator when Task 5 lands, against `a15fdcb1` and the merge's extension of it to the Owner targets). The plan is ready for implementation.

---

## Verdict

**0 blocking, 0 major, 0 minor.**  
Coverage: **INCOMPLETE**.

No design defect was demonstrated in the examined ground. This is not a claim that the plan compiles, passes tests, or is approved for implementation.

## Incorporation audit

| Prior finding | Assessment |
| --- | --- |
| M-1 — synchronous refusal could reenter the driver | **APPLIED.** Task 1 now specifies a per-device FIFO, enqueues inputs during a drain, and tests the synchronous refusal path ([plan, lines 73–80 and 98](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-23-phase-c0-stage-3a-ii-plan-dpms-execution.md:73)). |
| M-2 — differential assumed no DPMS event | **APPLIED.** Task 9 compares bytes for both a subscribed and an unsubscribed listener ([plan, lines 272–278](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-23-phase-c0-stage-3a-ii-plan-dpms-execution.md:272)), matching the corrected client contract ([spec, lines 349–365](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-23-phase-c0-stage-3a-arbiter-and-dpms-design.md:349)). |

The prior review marked all six round-one findings applied. This pass audited its two new findings; it did not reopen the six earlier assessments.

## Findings

### Blocking

None demonstrated.

### Major

None demonstrated.

### Minor

None demonstrated.

## Coverage and implementation checks

**24/24 bounded spec and source excerpts used.** For architecture and spec compliance, Task 3’s `ACTIVE`-only shape is expressible through `CommitDescription`: an `ACTIVE` CRTC object enters the closure, `CrtcPower` retains old and new power, an old-active CRTC enters `ExpectedCompletionCrtcs`, and the builder adds its required out-fence. The builder also supports `ALLOW_MODESET` ([plan, lines 125–134](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-23-phase-c0-stage-3a-ii-plan-dpms-execution.md:125); [spec, lines 164–172](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-23-phase-c0-stage-3a-arbiter-and-dpms-design.md:164); [closure.rs, lines 196–205](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/owner/closure.rs:196); [build.rs, lines 126–169](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/owner/build.rs:126)). This establishes a construction path, not hardware acceptance.

For safety and failure semantics, Task 3 addresses both observed bootstrap obstacles: dispatch currently refuses a missing measurement, and the deadline function rejects it. Its proposed `None → 30 s` rule matches C.0’s Bootstrap paragraph ([plan, lines 136–147](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-23-phase-c0-stage-3a-ii-plan-dpms-execution.md:136); [C.0, lines 2218–2229](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:2218); [device.rs, lines 1312–1316](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/owner/device.rs:1312); [deadlines.rs, lines 37–45](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/owner/deadlines.rs:37)).

Task 7’s read inventory is bounded by the named boolean and can be checked against source, but **its full Owner reachability classification remains unassessed**. Examined reads include direct eligibility at `3981`, M1 gating at `4954`, cursor animation at `7195` and `7230`, wakeup scheduling at `22508`, and composition at `22605` ([backend.rs](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/backend.rs:22508)). The search also located blackout at `23454`; that site and other matches were not individually classified. The merged COW restack path exempts COW direct frames from an unflip while still advancing layout generation, consistent with Task 5’s retained-direct and deferred-unflip rules ([backend.rs, lines 22191–22213](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/backend.rs:22191)). The **compose-timestamp flag interaction with Tasks 5–6 was not established**. Those two questions are the specific scope of any follow-up; unverified ground is not sound.

The plan assigns formatting, all-targets Clippy, compile-fail and behavioral tests, mutations, and a written but unrun hardware test to implementation ([plan, lines 279–305](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-23-phase-c0-stage-3a-ii-plan-dpms-execution.md:279)). This review ran no build or test.
