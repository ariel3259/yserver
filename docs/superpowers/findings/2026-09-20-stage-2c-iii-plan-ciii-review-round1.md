## Verdict

**2 blocking, 4 major, 0 minor**

**Coverage: COMPLETE FOR DECLARED SCOPE**

This is a design-review result only. It does not claim compilation, passing tests, or implementation approval.

## Incorporation audit

| Prior finding | Status | Result |
|---|---|---|
| None | N/A | No prior review was supplied; check 1 was skipped as directed. |

## Findings

### Blocking

#### B-1 — The owner-unflip request contract has no viable path to admission

The plan requires the request path to move `Current` into `ExitRetirement`, while readiness must remain `Waiting` whenever that position is occupied ([plan lines 44–52](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-20-phase-c0-stage-2c-iii-plan-ciii-unflip.md:44), [lines 178–180](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-20-phase-c0-stage-2c-iii-plan-ciii-unflip.md:178)). The baseline seam reserves that role and moves `Current` into it ([backend.rs lines 20900–20930](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/backend.rs:20900)); the snapshot then tests the same role for vacancy and reports `ExitRetirementOccupied` ([admission.rs lines 827–830](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/admission.rs:827), [lines 979–987](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/admission.rs:979)).

Concrete sequence: an unflip request moves the current resources into the exit role; the next wake sees that role occupied and can never admit the unflip that owns it. On materialization failure, the plan additionally promises retry on a later wake but names no wake-time operation that retries materialization.

There is also no production trigger contract: current causes still call `request_direct_unflip`, which only sets legacy `scanout_m2` flags ([backend.rs lines 2301–2325](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/backend.rs:2301)); the plan changes `admission_request_unflip` internally without assigning those causes a route into it. This conflicts with spec §6.1, which requires those causes to enter `admission_request_unflip` on an Owner device ([spec lines 481–489](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-19-phase-c0-stage-2c-iii-conversion-design.md:481)).

Smallest correction: define one per-device request entry used by every existing unflip cause, an explicit prepared-unflip state that distinguishes its own reserved exit role from foreign occupancy, and the exact wake-time retry/rollback transition.

#### B-2 — The copied-route fork is below the conductor and has no nonblocking copy-fence handoff

The spec requires every converted producer to enter the conductor and places the route fork at submit ([spec lines 72–73](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-19-phase-c0-stage-2c-iii-conversion-design.md:72), [lines 109–117](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-19-phase-c0-stage-2c-iii-conversion-design.md:109)). Task 7 instead puts the owner fork inside `PlatformBackend::submit_copied_scanout` ([plan lines 406–413](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-20-phase-c0-stage-2c-iii-plan-ciii-unflip.md:406)), below the scene path that creates conductor offers. Existing Owner completions are intercepted in the scene and queued as `ComposedOffer`; copied legacy completions proceed directly to the platform submit ([scene.rs lines 3783–3875](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/scene.rs:3783), [lines 3897–3902](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/scene.rs:3897)).

The sink copy returns an input-fence FD that legacy KMS consumes immediately ([platform.rs lines 6429–6456](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/platform.rs:6429)). Owner executor requests are sent as byte-only frames with no descriptor transfer ([executor/mod.rs lines 851–852](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/executor/mod.rs:851), [transport.rs lines 61–64](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/executor/transport.rs:61)). Thus the proposed fork can only bypass the conductor, pass a meaningless process-local FD, or block the event loop waiting for GPU completion.

Smallest correction: move copied preparation above the conductor boundary and define a value-owned pending-copy intent. Either wake admission nonblockingly after the fence signals, with cancellation/failure ownership specified, or explicitly extend owner IPC to transfer and patch input fences.

### Major

#### M-1 — Task 5 requires copied-route evidence before Task 7 makes it reachable

Task 5 includes the copied submit site and demands an Owner case for every enumerated site ([plan lines 325–349](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-20-phase-c0-stage-2c-iii-plan-ciii-unflip.md:325)). But Task 7 step 0 is the first permitted copied fixture, and Task 7 then removes copied-route refusal and implements its owner fork ([plan lines 387–408](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-20-phase-c0-stage-2c-iii-plan-ciii-unflip.md:387)).

At Task 5, a copied output cannot validly enter Owner; satisfying the test would require a prohibited fabricated fixture or eligibility bypass. Move the copied-site exclusivity case and its mutation to Task 7 after step 0, or reorder the tasks.

#### M-2 — The mandatory §6.4 hardware test has no implementation owner or executable test

Spec §6.4 requires one real-device test with P3-2/P3-3 mutations ([spec lines 511–538](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-19-phase-c0-stage-2c-iii-conversion-design.md:511)). The plan gives the coordinator prose steps but no task that creates the fixture/test and no test name or filter ([plan lines 428–461](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-20-phase-c0-stage-2c-iii-plan-ciii-unflip.md:428)). This cannot meet §8.2’s named-test requirement. Add an implementation task with an ignored hardware test, exact invocation, and separately applied P3-2/P3-3 mutations.

#### M-3 — Required portability gates are absent

Spec §8.4 requires `cargo check` for Linux glibc, Linux musl, and FreeBSD ([spec lines 645–653](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-19-phase-c0-stage-2c-iii-conversion-design.md:645)). The per-task gate lists builds, clippy, and tests but no cross-target checks ([plan lines 147–169](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-20-phase-c0-stage-2c-iii-plan-ciii-unflip.md:147)). Assign the three portability checks to implementation or final coordination.

#### M-4 — Cursor mutation proves representation, not preservation

The spec requires no cursor drop/flash and unchanged gamma ([spec lines 490–494](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-19-phase-c0-stage-2c-iii-conversion-design.md:490)). The plan instead requires explicit cursor-plane carriage and mutates by omitting it ([plan lines 188 and 304–315](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-20-phase-c0-stage-2c-iii-plan-ciii-unflip.md:188)). Existing legacy unflip updates only primary-plane properties while preserving the cursor ([modeset.rs lines 1714–1774](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/drm/modeset.rs:1714)); omission therefore does not violate the behavioral requirement. Replace T17 with an explicit cursor-disable mutation and assert the submitted request never disables or changes cursor state.

### Minor

None.

## Coverage and implementation checks

All four checks were performed. **21/24 bounded excerpt calls** were used; locator searches were excluded as directed. Verified ground covered unflip request/readiness/resource movement, copied completion routing and fence transport, task dependencies, §6.4 evidence, cursor semantics, and portability gates.

Not exhaustively assessed: the complete legacy-submit-site enumeration, all multi-device interactions through global `scanout_m2`/capacity state, full ledger restoration internals, and real copied-pool/card1 reachability. Those areas are unassessed, not declared sound.

Compilation, clippy, formatting, target checks, Vulkan execution, hardware tests, and mutation runs remain deferred to implementation as required.