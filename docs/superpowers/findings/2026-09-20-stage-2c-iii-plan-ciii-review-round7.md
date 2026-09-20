## Verdict

**0 blocking, 1 major, 0 minor**

Coverage: COMPLETE FOR DECLARED SCOPE

This is a design-review result only; it does not claim compilation, passing tests, or implementation approval.

## Incorporation audit

| Prior finding | Status | Result |
|---|---|---|
| Round-6 B-1 — ordinary direct-frame milestones lacked device-qualified correlation | **APPLIED** | Task 1 makes `CommitKey` mandatory for correlating consumers, explicitly migrates `managed_record_direct_presented` and `managed_enqueue_retired_direct_completion`, requires exact-key no-op behavior, and adds the foreign `Presented`/`CompletionRetired` sequence with A already sampled ([plan lines 445–505](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-20-phase-c0-stage-2c-iii-plan-ciii-unflip.md:445), [plan lines 517–521](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-20-phase-c0-stage-2c-iii-plan-ciii-unflip.md:517)). T44 specifically removes the retirement match ([plan line 427](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-20-phase-c0-stage-2c-iii-plan-ciii-unflip.md:427)). This closes the exact failure described in [round-6 B-1](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/findings/2026-09-20-stage-2c-iii-plan-ciii-review-round6.md:19). |

## Findings

### Blocking

None.

### Major

#### M-1 — The post-unflip re-entry barrier has no cumulative completion owner

Task 5 requires direct re-entry to remain blocked until a composed frame has been “presented on every affected output,” attributing that condition to `managed_can_enter_direct` ([plan lines 659–671](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-20-phase-c0-stage-2c-iii-plan-ciii-unflip.md:659)). But it defines neither the milestone that proves an individual output’s post-return full repaint nor state that accumulates those proofs.

The baseline clears `reentry_blocked_until_composed` only when one `scene.tick` returns a `composed_outputs` vector whose length equals the total output count ([backend.rs lines 21669–21686](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/backend.rs:21669)). That is a same-tick submission count, not cumulative per-output presentation evidence. Meanwhile, the plan’s test merely requires two outputs and per-output damage assertions; T19/T20 exercise invalidation content, not staggered completion or barrier discharge ([plan lines 414–415](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-20-phase-c0-stage-2c-iii-plan-ciii-unflip.md:414), [plan lines 673–678](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-20-phase-c0-stage-2c-iii-plan-ciii-unflip.md:673)).

Concrete failure: after unflip retirement, output A becomes ready and submits its full repaint on tick 1; output B does so on tick 2. Neither tick returns both outputs, so preserving the baseline clearing rule leaves direct re-entry blocked forever. Replacing it with a non-cumulative “any composed result” rule would instead permit re-entry before B’s required repaint, violating the specification that every affected composed buffer be fully repainted before it is scanned out again ([spec lines 481–494](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-19-phase-c0-stage-2c-iii-conversion-design.md:481)).

Smallest correction: Task 5 must choose the authoritative per-output proof—either post-return full-repaint submission, with an explicit owner-ordering argument, or a later owner milestone—and record the unflip’s affected-output set until each output supplies that proof. Add a staggered two-output test that keeps the barrier after A, clears it after B, and then permits re-entry, plus a mutation that clears after the first output.

### Minor

None.

## Coverage and implementation checks

All four requested checks were performed using **24/24 targeted spec/source excerpts**.

- **Incorporation:** Round-6 B-1 is fully incorporated at the type, consumer, test, and mutation levels.
- **Architecture/contracts:** Commit identity, event routing, request funnel, tick retry, admission dispatch, retirement ownership, multi-device boundaries, route exclusivity, and post-unflip re-entry were assessed.
- **Safety/ownership:** Equal-ID isolation, release-before-proof, capacity-role rollback, dispatch failure closure, resource movement, and return ordering were checked.
- **Spec/verification:** Relevant §§3.2, 6.1–6.4, and 8.2–8.4 were checked against tasks, mutations, hardware ownership, and portability gates.

Unassessed and not declared sound: exhaustive unflip-cause enumeration, every deliberately device-blind helper, fixture construction details, asynchronous rejection internals, and actual card1 retained-allocation reachability.

Compilation, formatting, clippy, cross-target checks, Vulkan/hardware execution, and mutation runs remain deferred to implementation.