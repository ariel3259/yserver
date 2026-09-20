## Verdict

**1 blocking, 0 major, 0 minor**

Coverage: COMPLETE FOR DECLARED SCOPE

This is a design-review result only. It does not claim compilation, passing tests, or implementation approval.

## Incorporation audit

| Prior finding | Status | Result |
|---|---|---|
| Round-5 B-1 — identity inventory omitted scene commit consumers | **APPLIED** | Task 1 now includes `scene.rs`, qualifies damage transactions and owner-buffer transitions by `(DrmDeviceKey, CommitId)`, requires later recorded identities—including unflip retirement—to use that key, and adds the interleaved A/B scene test with T42/T43 ([plan lines 412–480](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-20-phase-c0-stage-2c-iii-plan-ciii-unflip.md:412)). This addresses the exact scene omission from round 5. |

## Findings

### Blocking

#### B-1 — Direct-frame retirement still has no device-qualified correlation

Task 1 identifies the pending direct frame only in connection with `managed_enqueue_unknown_direct_completion`, and its test exercises a foreign `Terminal`/`CompletionUnknown` ([plan lines 423–446](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-20-phase-c0-stage-2c-iii-plan-ciii-unflip.md:423), [plan lines 466–480](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-20-phase-c0-stage-2c-iii-plan-ciii-unflip.md:466)). It does not assign qualified correlation to the pending frame’s normal `Presented` and `CompletionRetired` consumers.

The current routing demonstrates the missing contract:

- `Presented` passes only the numeric commit to `managed_record_direct_presented` ([backend.rs lines 20328–20358](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/backend.rs:20328)), which matches the pending frame using only `CommitId` ([backend.rs lines 2844–2855](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/backend.rs:2844)).
- More seriously, after consuming any `CompletionRetired`, routing calls `managed_enqueue_retired_direct_completion()` with neither device nor commit ([backend.rs lines 20401–20465](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/backend.rs:20401)). That helper unconditionally takes the pending direct frame; if it already has a presentation sample, it publishes its completion, promotes it to current, and releases the preceding frame ([backend.rs lines 2872–2899](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/backend.rs:2872)).

Concrete failure: device A has a pending direct commit `(A, 1)` whose `Presented` sample has arrived, but whose retirement has not. Device B’s commit `(B, 1)` retires first. Task 1’s corrected resource consumer legitimately consumes B’s qualified record, after which the unconditional helper publishes A’s Present and releases A’s preceding current frame without A’s retirement proof. A foreign `Presented` can also populate A’s sample when numeric commit and CRTC handles collide. This violates the requirement that an event on one device change nothing on another ([spec lines 496–500](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-19-phase-c0-stage-2c-iii-conversion-design.md:496)) and permits release-before-proof.

The named equal-ID resource test, foreign-terminal test, and scene test can all pass without exercising this normal direct-frame retirement path.

Smallest correction: add every pending-direct milestone consumer to Task 1’s identity inventory. Record the qualified identity on the frame; pass `(device, commit)` to both `managed_record_direct_presented` and `managed_enqueue_retired_direct_completion`; and make both no-op unless that exact identity matches. Extend the foreign-milestone test with an already-sampled pending A frame followed by B’s `Presented` and `CompletionRetired`, asserting A’s pending/current state, publication queue, and pins remain unchanged. Broaden T41 or add a dedicated mutation for removing the retirement match.

### Major

None.

### Minor

None.

## Coverage and implementation checks

All four requested checks were performed using **24/24 targeted spec/source excerpts**.

- **Incorporation:** the round-5 scene correction is applied; the finding above is a separate omitted direct-frame consumer.
- **Architecture/contracts:** request funnel, tick retry, admission dispatch/failure handling, event routing, qualified resource/scene identity, direct-frame publication, multi-device topology, and unflip retirement ownership were assessed.
- **Safety/ownership:** equal-ID ordering, release proof, capacity rollback, scene ownership, failure closure, and pending-frame terminalization were checked.
- **Spec/verification:** authoritative §§3.2, 6.1–6.4, and 8.2–8.4 were checked against tasks, tests, mutations, hardware ownership, and portability gates.

Unassessed and not declared sound: exhaustive unflip-cause enumeration, every device-blind helper, fixture construction details, partial side effects of failed shadow materialization, and real-card retained-allocation reachability.

Compilation, formatting, clippy, cross-target checks, Vulkan/hardware execution, and mutation runs remain deferred to implementation.