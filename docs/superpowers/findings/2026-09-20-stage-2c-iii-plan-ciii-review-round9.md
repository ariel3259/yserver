## Verdict

**1 blocking, 1 major, 0 minor**

Coverage: COMPLETE FOR DECLARED SCOPE

The plan is **not ready for implementation**. This is a design-review result only; it does not claim compilation, passing tests, or implementation approval.

## Incorporation audit

| Prior finding | Status | Result |
|---|---|---|
| Round-8 M-1 — cumulative barrier lacked an authoritative presentation-proof handoff | **APPLIED** | Task 5 now makes the scene the producer, keys proof by `CommitKey` and output, emits only after successful damage application, and excludes submission and invalidation branches ([plan line 701](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-20-phase-c0-stage-2c-iii-plan-ciii-unflip.md:701), [plan line 720](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-20-phase-c0-stage-2c-iii-plan-ciii-unflip.md:720)). T46/T47 and the withheld-completion and invalidated-member tests exercise those distinctions ([plan line 748](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-20-phase-c0-stage-2c-iii-plan-ciii-unflip.md:748)). This also closes round-7 M-1. |
| Round-8 M-2 — hardware sequence had no retaining commit | **TRADED** | Task 8 now provides the reachable retaining shape: a second direct commit using the same allocation while the first remains current ([plan line 848](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-20-phase-c0-stage-2c-iii-plan-ciii-unflip.md:848)). However, moving P3-3 before the unflip now contradicts the authoritative §6.4 sequence and the plan’s own exit table. M-1 below remains. |

## Findings

### Blocking

#### B-1 — Task 5’s return effects are not scoped to the owning device

The spec requires an event on one device to change nothing on another ([spec line 496](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-19-phase-c0-stage-2c-iii-conversion-design.md:496)). The plan similarly declares that a device-blind path changing another device’s damage state is a defect ([plan line 338](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-20-phase-c0-stage-2c-iii-plan-ciii-unflip.md:338)).

Task 5 nevertheless places its effects beside the legacy return and requires it to “mark the scene structure dirty,” without defining a scoped replacement or testing an unaffected device ([plan line 699](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-20-phase-c0-stage-2c-iii-plan-ciii-unflip.md:699)). The cited legacy return calls both global helpers ([backend.rs line 3355](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/backend.rs:3355)). `invalidate_all_scanout_damage` invalidates every scene output ([scene.rs line 1491](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/scene.rs:1491)), while `mark_scene_structure_dirty` adds full-output structure damage to every output ([scene.rs line 2691](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/scene.rs:2691)).

Concrete failure: devices A and B have scene outputs, and A’s device-local direct unit completes its unflip. Using the named baseline effects invalidates B’s scanout history and adds full structural damage to B, causing an unrelated repaint from A’s event. Task 2 runs before this path exists, while Task 5’s tests contain only affected outputs, so the violation survives the proposed evidence.

Smallest correction: define one Task-5 scene operation taking the recorded affected output identities and applying invalidation/structural damage only to that set. Add an A/B case asserting B’s damage state remains byte-for-byte unchanged after A’s unflip retirement, with a mutation that widens the operation to all outputs.

### Major

#### M-1 — The accepted P3-3 correction no longer matches the authoritative specification

The authoritative §6.4 sequence requires composed → direct → unflip → retaining commit, identifies P3-2 with steps 1–3 and P3-3 with step 4 ([spec line 511](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-19-phase-c0-stage-2c-iii-conversion-design.md:511)). Revision 9 instead correctly makes the retaining commit step 3 and the unflip step 4 ([plan line 848](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-20-phase-c0-stage-2c-iii-plan-ciii-unflip.md:848)). The plan’s exit table still says P3-3 is step 4 ([plan line 474](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-20-phase-c0-stage-2c-iii-plan-ciii-unflip.md:474)).

Consequently, implementation prose, authoritative acceptance criteria, and coordinator mutation accounting disagree about which commit proves P3-3. Compiler and hardware success cannot resolve which contract governs acceptance.

Smallest correction: amend authoritative §6.4 and the plan’s exit table to the accepted reachable sequence—P3-3 at the second direct commit, followed by the unflip—and update the P3-2 step references accordingly.

### Minor

None.

## Coverage and implementation checks

All four checks were performed using **24/24** bounded spec/source excerpts beyond the once-read plan and prior review.

- **Incorporation:** both round-8 findings were traced through task text, tests, and mutations.
- **Architecture/contracts:** commit qualification, per-device ownership, request/dispatch division, retirement effects, proof handoff, route exclusivity, and hardware sequencing were assessed.
- **Safety/ownership:** event ordering, invalidated-member handling, cumulative barriers, dispatch rollback, resource retention, and cross-device damage isolation were checked.
- **Spec/verification:** authoritative §§3.2, 6.1–6.4, and 8.1–8.4 were compared with the plan; implementation gates are assigned.

Unassessed and not declared sound: exhaustive unflip-cause enumeration, every deliberate device-blind helper, exact fixture construction, actual card1 reachability, copied-route work, and stages 3/4. Rust signatures, borrow/trait issues, builds, formatting, clippy, cross-target checks, mutation compilation, and GPU execution remain for implementation.