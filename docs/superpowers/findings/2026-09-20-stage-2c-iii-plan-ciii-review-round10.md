## Verdict

**0 blocking, 1 major, 0 minor**

Coverage: COMPLETE FOR DECLARED SCOPE

The plan is **not ready for implementation** until the narrow verification gap below is resolved. This is a design-review result only; it does not claim compilation, passing tests, or implementation approval.

## Incorporation audit

| Prior finding | Status | Result |
|---|---|---|
| Round-9 B-1 — return effects were device-blind | **APPLIED** | Task 5 replaces both global legacy helpers with one scene operation over recorded affected-output identities, preserves the global helpers only for Legacy, adds the A/B isolation case, and assigns T48 ([plan line 727](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-20-phase-c0-stage-2c-iii-plan-ciii-unflip.md:727), [plan line 734](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-20-phase-c0-stage-2c-iii-plan-ciii-unflip.md:734), [plan line 788](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-20-phase-c0-stage-2c-iii-plan-ciii-unflip.md:788)). |
| Round-9 M-1 — P3-3 step disagreement | **APPLIED** | The authoritative annotation now establishes composed → direct → retaining direct → unflip ([spec line 527](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-19-phase-c0-stage-2c-iii-conversion-design.md:527)). Task 8 and the exit table consistently assign P3-3 to step 3 and P3-2 to the displacing steps 1, 2, and 4 ([plan line 883](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-20-phase-c0-stage-2c-iii-plan-ciii-unflip.md:883), [plan line 495](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-20-phase-c0-stage-2c-iii-plan-ciii-unflip.md:495)). |

## Findings

### Blocking

None.

### Major

#### M-1 — The T48 A/B test cannot drive the real unflip route in the topology it requires

Task 5 says devices A and B both have scene outputs and that A’s unflip retires, after which B must remain unchanged ([plan line 788](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-20-phase-c0-stage-2c-iii-plan-ciii-unflip.md:788)). Task 4 simultaneously makes `direct_scanout_topology_eligible` a mandatory dispatch precondition ([plan line 686](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-20-phase-c0-stage-2c-iii-plan-ciii-unflip.md:686)).

The baseline predicate returns true only when **every** platform output belongs to the primary device ([backend.rs line 3457](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/backend.rs:3457)). With an output on device B, device A cannot enter the grouped direct route and therefore cannot produce the real unflip whose retirement the named test claims to exercise.

Concrete consequence: keeping B present makes the unflip undispatchable; removing B makes widening the operation to every scene output observationally identical to the correct A-only set. A test that directly invokes the new scene operation could catch T48, but it would not itself prove the claimed retirement routing and would conflict with the plan’s prohibition on tests bypassing the path they name ([plan line 418](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-20-phase-c0-stage-2c-iii-plan-ciii-unflip.md:418)). This is precisely the kind of unreachable scenario the plan classifies as an F8 stop.

Smallest correction: explicitly make the evidence compositional—one real single-device retirement-routing test proving that the dispatch-recorded identities reach the scoped operation, plus a separately named two-device component test proving that operation leaves B unchanged and catches T48. State that the A/B test is component-level rather than claiming A’s real unflip dispatched in a multi-device topology.

### Minor

None.

## Coverage and implementation checks

All four checks were performed using **24/24** bounded logical spec/source excerpts beyond the once-read plan and prior review.

- **Incorporation:** both round-9 findings were traced through task text, tests, mutations, the corrected exit table, and the dated §6.4 annotation.
- **Architecture/contracts:** commit qualification, per-device ownership, request/dispatch separation, return-proof delivery, route exclusivity, and hardware sequencing were assessed.
- **Safety/ownership:** retirement ordering, invalidated-member behavior, cumulative barriers, dispatch rollback, resource retention, and cross-device damage isolation were checked.
- **Spec/verification:** §§3.2, 6.1–6.4, 8.1–8.4 and the assigned build, mutation, portability, Vulkan, and hardware gates were compared with the plan.

Unassessed and not declared sound: exhaustive unflip-cause enumeration, every deliberate device-blind helper, exact fixture construction, actual card1 reachability, copied-route work, and stages 3/4. Rust signatures, borrow/trait behavior, builds, formatting, clippy, cross-target checks, mutation compilation, and GPU execution remain for implementation.