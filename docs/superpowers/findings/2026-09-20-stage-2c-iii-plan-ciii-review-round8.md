## Verdict

**0 blocking, 2 major, 0 minor**

Coverage: COMPLETE FOR DECLARED SCOPE

This is a design-review result only; it does not claim compilation, passing tests, or implementation approval.

## Incorporation audit

| Prior finding | Status | Result |
|---|---|---|
| Round-7 M-1 — no cumulative post-unflip completion owner | **PARTIAL** | Revision 8 adds the affected-output set, cumulative discharge, staggered test, and T45 ([plan lines 683–715](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-20-phase-c0-stage-2c-iii-plan-ciii-unflip.md:683)). However, round 7 required choosing the authoritative per-output proof ([round-7 lines 23–31](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/findings/2026-09-20-stage-2c-iii-plan-ciii-review-round7.md:23)). Revision 8 instead leaves the implementer to “name the milestone” or stop. It defines neither the producer-to-barrier handoff nor evidence distinguishing submission from presentation. M-1 carries this forward. |

## Findings

### Blocking

None.

### Major

#### M-1 — The cumulative barrier still lacks a per-output presentation-proof handoff

The spec requires every affected buffer to be fully repainted before it is scanned out again ([spec lines 481–494](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-19-phase-c0-stage-2c-iii-conversion-design.md:481)). Task 5 now requires on-screen proof but leaves its milestone and integration contract undecided ([plan lines 683–704](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-20-phase-c0-stage-2c-iii-plan-ciii-unflip.md:683)); `scene.rs` is not among the task’s files ([plan lines 672–676](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-20-phase-c0-stage-2c-iii-plan-ciii-unflip.md:672)).

A bare owner milestone is insufficient. `HardwareComplete` carries only a commit id ([device.rs lines 41–54](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/owner/device.rs:41)). The scene privately owns the exact output/generation transaction and consumes it at `HardwareComplete` ([scene.rs lines 1891–1958](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/scene.rs:1891), [scene.rs lines 2357–2398](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/scene.rs:2357)). The backend routes the event through the scene before its own match ([backend.rs lines 20208–20219](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/backend.rs:20208)). Moreover, `HardwareComplete` can leave a member invalidated rather than successfully applied when its submitted generation cannot be confirmed ([scene.rs lines 2377–2397](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/scene.rs:2377)).

Concrete failure: A submits after the unflip. Clearing A at submission violates the presentation requirement. Clearing A on any matching `HardwareComplete` can also be wrong when the scene invalidates that transaction member. Waiting for information that the scene discards leaves `{A,B}` permanently nonempty.

The staggered test does not close this: an implementation that removes each output at submission still holds after tick A and clears after tick B, exactly the stated assertions; T45 catches clearing everything after A, not submission-before-proof ([plan lines 712–715](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-20-phase-c0-stage-2c-iii-plan-ciii-unflip.md:712)).

Smallest correction: define a scene-produced proof keyed by `CommitKey` and output identity, emitted only after successful full-repaint application at `HardwareComplete`, and define how Task 5 consumes it. The test must withhold `HardwareComplete` after submission, prove the barrier remains, then deliver A and B’s proofs separately; an invalidated member must not discharge its output. Add a mutation that discharges at submission.

#### M-2 — The hardware P3-3 sequence does not identify a retaining commit

The spec requires the plan to choose a reachable commit whose new state retains an old-state allocation for the same member ([spec lines 520–525](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-19-phase-c0-stage-2c-iii-conversion-design.md:520)). Task 8 instead repeats the unverified example “a direct Present of the same source buffer again” after the unflip ([plan lines 808–821](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-20-phase-c0-stage-2c-iii-plan-ciii-unflip.md:808)).

In the stated order, step 3’s retired unflip makes its new composed resources current ([commit.rs lines 300–350](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/resources/commit.rs:300)). A following direct dispatch takes those current resources as `old` ([admission.rs lines 1521–1534](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/admission.rs:1521)). Reusing step 2’s direct source therefore produces old=composed and new=direct; it does not retain that direct allocation from old state. Registration recognizes retention only when the same allocation key occurs in old and new for the same member ([commit.rs lines 654–668](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/resources/commit.rs:654)).

Consequently, the proposed example cannot exercise P3-3 or T33; the implementation would discover reachability only in the final hardware task.

Smallest correction: specify a verified sequence and name the retained allocation/member—for example, after step 3, establish direct D as current, then submit a second reachable direct commit retaining D, if that route is confirmed. Distinguish the setup commit from the P3-3 commit and assert both absence of D’s obligation and normal registration of any displaced allocation.

### Minor

None.

## Coverage and implementation checks

All four checks were performed using **23/24** targeted spec/source excerpts beyond the once-read plan and prior review.

- **Incorporation:** Round-7 M-1 is partial as described.
- **Architecture/contracts:** Commit identity, device isolation, request/readiness, dispatch/rollback, retirement ownership, proof routing, exclusivity, and hardware sequencing were assessed.
- **Safety/ownership:** Equal-id correlation, resource-state transitions, release proof, cumulative ordering, failure closure, and retained-allocation identity were checked.
- **Spec/verification:** Relevant §§3.2, 6.1–6.4, and 8.2–8.4 were checked. The cited source files are unchanged from baseline `5a34c6ec`.

Unassessed and not declared sound: exhaustive cause enumeration, every deliberate device-blind helper, fixture construction details, copied-route work, stage 3/4 behavior, and actual card1 reachability. Exact Rust APIs, compiler/borrow issues, builds, clippy, cross-target checks, mutation compilation, and GPU execution remain for implementation.