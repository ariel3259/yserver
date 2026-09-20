## Verdict

**2 blocking, 1 major, 0 minor**

**Coverage: COMPLETE FOR DECLARED SCOPE**

This is a design-review result only. It does not claim that code compiles, tests pass, or implementation is approved.

## Incorporation audit

| Prior finding | Status | Result |
|---|---|---|
| R1 B-1 — unflip request could not reach admission | **TRADED** | Capacity movement and retry were reassigned, but the new request edge forms a cycle with the existing admission callback. See B-1. |
| R1 B-2 — copied-route fence/conductor mismatch | **APPLIED** | The copied route and its fixture were moved to a separate plan, with Owner entry forbidden until then ([plan 201–210](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-20-phase-c0-stage-2c-iii-plan-ciii-unflip.md:201)). |
| R1 M-1 — copied-route evidence before reachability | **APPLIED** | The copied-route exclusivity case left this plan with the route itself ([plan 440–456](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-20-phase-c0-stage-2c-iii-plan-ciii-unflip.md:440)). |
| R1 M-2 — no executable §6.4 test | **APPLIED** | Task 7 owns a named, coordinator-run hardware test outside routine filters ([plan 519–548](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-20-phase-c0-stage-2c-iii-plan-ciii-unflip.md:519)). |
| R1 M-3 — portability gates absent | **APPLIED** | All three required target checks are assigned to Task 7 and acceptance ([plan 267–273](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-20-phase-c0-stage-2c-iii-plan-ciii-unflip.md:267)). |
| R1 M-4 — cursor preservation backwards | **APPLIED** | The transaction omits cursor and gamma properties; T22/T23 add them ([plan 420–435](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-20-phase-c0-stage-2c-iii-plan-ciii-unflip.md:420)). |
| R2 B-1 — routine filter selected tty-only test | **APPLIED** | It is renamed `c0_hw_ciii_owner_route_on_card1_drm`, outside every permitted implementation filter ([plan 188–199](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-20-phase-c0-stage-2c-iii-plan-ciii-unflip.md:188)). |
| R2 M-1 — exclusivity lacked an independent observation | **APPLIED** | Task 5 requires a sink-entry record before authorization and tests both permitted and refused entries ([plan 458–483](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-20-phase-c0-stage-2c-iii-plan-ciii-unflip.md:458)). |

## Findings

### Blocking

#### B-1 — The specified Owner request path recursively calls itself

The plan makes `request_direct_unflip` the sole producer entry and directs its Owner branch through `admission_request_unflip` ([plan 127–135](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-20-phase-c0-stage-2c-iii-plan-ciii-unflip.md:127), [plan 319–325](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-20-phase-c0-stage-2c-iii-plan-ciii-unflip.md:319)). Baseline `admission_request_unflip`, however, calls `request_direct_unflip` after installing the admission intent ([admission.rs 713–733](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/admission.rs:713)).

Concrete sequence: an Owner-side cause enters `request_direct_unflip`; its new fork calls `admission_request_unflip`; that function calls `request_direct_unflip`; the Owner fork repeats before either call returns. No request reaches a stable wake/readiness state.

Smallest correction: define an acyclic ownership boundary. `request_direct_unflip` should perform legacy flag changes and call an admission-only primitive that never calls back into the funnel; replace or remove the existing reverse callback and identify how its current test callers obtain any required legacy effects.

#### B-2 — Per-owner commit IDs collide in the global resource consumer

Section 6.2 requires one device’s events to change nothing on another ([spec 496–500](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-19-phase-c0-stage-2c-iii-conversion-design.md:496)). Each owner’s allocator starts commit numbering at 1 ([identity.rs 137–180](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/owner/identity.rs:137)), but `KmsBackend` has one global `CommitResourceConsumer` ([backend.rs 1498–1501](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/backend.rs:1498)). Its completion, member and retirement maps are keyed solely by `CommitId` ([commit.rs 130–145](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/resources/commit.rs:130)).

Concrete sequence: device A’s commit 1 receives early `HardwareComplete`, which is cached under key `1`; before A retires, device B’s commit 1 produces `CompletionRetired`. B removes A’s cached completion and discharges B’s KMS obligations as completed ([commit.rs 282–312](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/resources/commit.rs:282)). B’s displaced allocation can therefore be released without B’s real completion, violating §3.2 and P3-2 ([spec 143–152](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-19-phase-c0-stage-2c-iii-conversion-design.md:143), [spec 527–532](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-19-phase-c0-stage-2c-iii-conversion-design.md:527)). Task 6 does not require overlapping equal numeric commit IDs or inspect resource obligations, so its four cases do not establish this contract.

Smallest correction: namespace every commit-correlation operation by `(DrmDeviceKey, CommitId)`—including completion caches, members, reserved retirements and rejected-resource lookup—or establish per-device consumers. Add an interleaved two-owner test using equal numeric commit IDs that proves neither device can discharge or restore the other’s resources.

### Major

#### M-1 — Failed unflip dispatch has no capacity-role rollback contract

The plan moves `Current` into `ExitRetirement` during dispatch, before the fallible owner operation completes ([plan 107–125](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-20-phase-c0-stage-2c-iii-plan-ciii-unflip.md:107)). The existing seam performs that mutation immediately ([backend.rs 20900–20930](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/backend.rs:20900)), while the shared failure handler merely returns old resources to `current_resources` without restoring their role ([admission.rs 1142–1167](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/admission.rs:1142)). Task 2’s failure test checks only gate closure, absence of legacy writes and retained pins ([plan 381–383](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-20-phase-c0-stage-2c-iii-plan-ciii-unflip.md:381)).

Concrete sequence: role movement succeeds; ledger construction or send fails; the direct buffer remains on hardware and its resources return to the current collection, but its capacity token still says `ExitRetirement`. The later recovery handoff begins from ownership state that contradicts hardware reality.

Smallest correction: specify an unflip failure row that atomically restores the old resource to `Current`, or delays irreversible role movement until confirmed dispatch while preserving the retirement reservation. Extend the named failure test to assert role/capacity state, not only pin retention. This does not require stage-4 gate recovery.

### Minor

None.

## Coverage and implementation checks

All four required checks were performed using **24/24 bounded spec/source excerpts**. Verified ground included both review-round corrections, unflip request/readiness/dispatch/retirement flow, resource-consumer correlation, multi-device identity, route-exclusivity evidence, hardware ownership checks, mutations, and portability gates.

Unassessed and not declared sound: exhaustive device-blind layout paths, real card1 retained-allocation reachability, detailed fixture construction, and other non-cited submit sites. The excerpt budget is exhausted.

Compilation, formatting, clippy, cross-target checks, Vulkan/hardware execution, and mutation runs remain deferred to implementation.