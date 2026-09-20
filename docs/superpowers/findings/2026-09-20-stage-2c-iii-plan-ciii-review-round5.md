## Verdict

**1 blocking, 0 major, 0 minor**

Coverage: COMPLETE FOR DECLARED SCOPE

This is a design-review result only; it does not claim compilation, passing tests, or implementation approval.

## Incorporation audit

| Round-4 finding | Status | Result |
|---|---|---|
| B-1 — materialization retry lacked a tick edge | **APPLIED** | The retry is now assigned to the `maybe_composite` transaction fork, retries materialization without repeating intent/terminalization, and T39 requires a one-shot cause followed by an ordinary tick ([plan lines 180–189](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-20-phase-c0-stage-2c-iii-plan-ciii-unflip.md:180), [plan lines 410–422](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-20-phase-c0-stage-2c-iii-plan-ciii-unflip.md:410)). The current tick reaches that fork while a current direct frame and unflip request exist ([backend.rs line 21564](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/backend.rs:21564)). |
| M-1 — bare `CommitId` consumers outside the resource maps | **PARTIAL** | Task 7 now covers Present dispositions, event routing, and the pending direct frame ([plan lines 621–637](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-20-phase-c0-stage-2c-iii-plan-ciii-unflip.md:621)). However, its claimed full inventory omits the shared scene damage map and owner-buffer consumers described in B-1 below. |

## Findings

### Blocking

#### B-1 — Task 7’s “full” identity inventory omits shared scene commit consumers

Task 7 makes its listed sites the scope boundary and limits its files to resource handling, Present handling, and backend routing ([plan lines 610–637](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-20-phase-c0-stage-2c-iii-plan-ciii-unflip.md:610)). Task 6 explicitly defers commit identity to Task 7 ([plan lines 582–584](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-20-phase-c0-stage-2c-iii-plan-ciii-unflip.md:582)).

The existing scene nevertheless has additional backend-wide numeric-ID correlations:

- `owner_damage_transactions` is keyed solely by `CommitId`; installation rejects an already-present number ([scene.rs line 1080](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/scene.rs:1080), [scene.rs line 1895](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/scene.rs:1895)).
- Routing receives `device`, but `Accepted`, `HardwareComplete`, retirement, and terminal paths call damage and owner-buffer consumers with only the commit number ([scene.rs lines 1964–2004](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/scene.rs:1964)).
- Owner-buffer transitions scan every output for `buffer.commit_id() == Some(commit)` without filtering by device ([scene.rs line 2085](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/scene.rs:2085), [scene.rs line 2175](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/scene.rs:2175), [scene.rs line 2217](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/scene.rs:2217)).

Concrete failure: owners A and B both mint commit 1. A installs a composed damage transaction and submitted owner buffer. B’s `Accepted { commit: 1 }` then accepts A’s damage transaction and buffer; B’s `HardwareComplete { commit: 1 }` can remove and apply A’s transaction. Independently, simultaneous installation of B’s own transaction is rejected merely because A already occupies numeric key 1. Thus an event on B changes or prevents A’s work, directly violating §6.2’s isolation invariant ([spec lines 496–500](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-19-phase-c0-stage-2c-iii-conversion-design.md:496)).

The specified equal-ID tests exercise the resource cache and pending Present, not scene transactions or owner buffers ([plan lines 652–661](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-20-phase-c0-stage-2c-iii-plan-ciii-unflip.md:652)); therefore all named evidence can pass while this defect remains.

Smallest correction: add `scene.rs` to Task 7 and qualify the damage-transaction key and every owner-buffer event scan with `(DrmDeviceKey, CommitId)`. Require Task 3’s newly recorded unflip-retirement identity to use that same qualified key. Extend the equal-ID test with live A/B scene transactions and buffers, interleaving B’s Accepted, HardwareComplete, Terminal, and CompletionRetired events while asserting A remains untouched; add mutations that remove the device from the map lookup and buffer scan.

### Major

None.

### Minor

None.

## Coverage and implementation checks

All four requested checks were performed using **24/24 bounded spec/source excerpts**.

- Incorporation: both round-4 findings were audited; B-1 is applied and M-1 remains partial.
- Architecture/contracts: request funnel, tick retry, admission wake/dispatch, event routing, shared consumers, and cross-task identity ownership were checked.
- Safety/failure semantics: capacity rollback, direct-frame pin terminalization, equal-ID ordering, damage ownership, and cross-device event effects were assessed.
- Spec/verification: §§3.2, 3.3, 6.1–6.4, and 8.1–8.4 were checked against the tasks, tests, mutations, hardware assignment, and portability gates.

Unassessed and not declared sound: exhaustive unflip-cause call sites, every device-blind helper, fixture construction details, partial-side-effect behavior inside failed GPU shadow materialization, and real-card retained-allocation reachability. The excerpt budget is exhausted.

Compilation, formatting, clippy, cross-target checks, Vulkan/hardware execution, and mutation runs remain deferred to implementation.