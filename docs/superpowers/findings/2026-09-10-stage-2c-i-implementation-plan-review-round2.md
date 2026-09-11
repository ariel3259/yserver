## Verdict

**1 blocking, 2 major, 0 minor**

**Coverage: COMPLETE FOR DECLARED SCOPE**

This is a design-review result only; it does not claim compilation, passing tests, or implementation approval.

## Incorporation audit

| Prior finding | Status | Assessment |
|---|---|---|
| B-1 — non-transactional GPU proof application | **APPLIED** | Task 5 now validates the complete batch without mutation, commits through an infallible/non-reentrant phase, roots the intact batch before quarantine, and requires valid-first/stale-later plus freeze-failure regressions ([plan lines 336–359](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-09-phase-c0-stage-2c-i-resource-terminalization.md:336)). |
| M-1 — no authoritative capacity completion path | **APPLIED** | Task 8 places the sole capacity table in `CommitResourceConsumer`, routes availability to `on_available`, keeps tokens in resource records, and defines consuming completion operations and delayed-release tests ([plan lines 522–524](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-09-phase-c0-stage-2c-i-resource-terminalization.md:522), [line 546](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-09-phase-c0-stage-2c-i-resource-terminalization.md:546)). |
| M-2 — writer tests checked only the enum | **APPLIED** | Task 6 inventories real primary, lifecycle, cursor, gamma, and helper entry points, places counting observations below those boundaries, and Task 10 requires sink/caller classification ([plan lines 410–421](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-09-phase-c0-stage-2c-i-resource-terminalization.md:410), [lines 617–628](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-09-phase-c0-stage-2c-i-resource-terminalization.md:617)). A separate future-owner authorization defect remains below; it does not negate the correction to legacy enforcement. |
| m-1 — raw CRTC group membership | **APPLIED** | `GroupMember` now carries `CrtcKey`, topology generation, and CRTC epoch, and matching is forbidden from consulting replacement topology ([plan lines 434–453](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-09-phase-c0-stage-2c-i-resource-terminalization.md:434)). |

## Findings

### Blocking

#### B-1 — Teardown release omits the outstanding KMS/current-scanout disposition

Task 9 defines `TeardownRelease` as validating file-owned, GPU/read, and FOREIGN dispositions before ending quarantine, but does not include KMS obligations or a device/current-scanout barrier ([plan lines 567–569](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-09-phase-c0-stage-2c-i-resource-terminalization.md:567)). This conflicts with the authoritative availability conjunction, where applicable KMS obligations remain independent and neither fd closure nor unrelated evidence fabricates their proof ([resource spec lines 190–199](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-08-phase-c0-stage-2c-i-resource-terminalization-design.md:190)), and with the requirement that accepted-buffer release await `PriorBufferReleased` or a teardown barrier proving the buffer unreachable ([governing spec lines 2228–2233](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:2228)).

Concrete sequence: an acceptance-unknown direct commit may still have installed the new framebuffer. The helper reaps, the registered fd family closes, and GPU/read/FOREIGN dependencies finish. The listed `TeardownRelease` predicates all pass while the allocation’s KMS/current-state disposition remains unknown. If release clears quarantine, it can signal release or destroy userspace ownership without proof of KMS unreachability; if the KMS obligation is preserved, the proposed capability cannot complete valid teardown and leaks indefinitely.

Smallest correction: make teardown validation require an exact commit/CRTC-generation KMS disposition, or a typed device-loss/full-teardown proof that normatively supersedes it. Add a regression where every currently listed predicate is satisfied but unresolved KMS ownership still rejects teardown release.

### Major

#### M-1 — The future Owner writer capability has no defined authority contract

The produced `TransportGate` API exposes only `allows_legacy`; `HandoverPermit` authorizes route publication, not an individual mutation ([plan lines 379–393](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-09-phase-c0-stage-2c-i-resource-terminalization.md:379)). Task 6 later mentions a “separate qualified permit” for owner-mediated dispatch without defining its issuer, device/incarnation binding, writer class, consumption, or revocation semantics ([plan line 410](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-09-phase-c0-stage-2c-i-resource-terminalization.md:410)).

Production writers are deferred, but their activation interface is in scope. The governing 2c contract requires every mutating entry to check the single incarnation-bound authority and requires future qualified mutations to remain owner-authorized ([conversion spec lines 264–280](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-08-phase-c0-stage-2c-conversions-and-damage-design.md:264)). As written, a later stage must invent this security boundary and could treat `state == Owner` as blanket permission, allowing an uncorrelated lifecycle or helper mutation.

Smallest correction: define the typed owner-writer permission contract now: serialized issuer, device/incarnation and writer-class/transaction binding, validation at the final sink, non-replay/consumption rules, and helper revocation behavior. Concrete stage-3/4 producers may remain deferred.

#### M-2 — Overlay re-claim lacks a physical-retirement transition contract

Task 7 correctly separates `ServerState::cow_claims` from physical leases, but says only that a new claim during deferred release must use the retained identity “correctly” ([plan line 492](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-09-phase-c0-stage-2c-i-resource-terminalization.md:492)). It defines no edge delivery from the logical claim owner to the resource consumer, no generation correlation, and no decision for cancelable versus already-dispatched unflip.

Concrete sequence: logical count reaches zero, physical COW enters delayed unflip/retirement, then the same drawable is claimed again before replacement evidence. Without a specified transition, implementation can either create a second physical owner/import or revive the retained lease while its old KMS-release obligation remains armed, allowing later evidence to retire an actively reclaimed resource. The authoritative addition requires re-claim of the retained identity without resurrecting claims from leases ([resource spec lines 108–114](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-08-phase-c0-stage-2c-i-resource-terminalization-design.md:108)).

Smallest correction: define a generation-keyed 0→1/1→0 consumer transition, including pending-versus-dispatched unflip handling and the failure route to sticky `cow_teardown_failed`. Require the re-claim regression to assert no duplicate import, no stale release, and exactly one logical claim authority.

### Minor

None.

## Coverage and implementation checks

- Incorporation: all four prior findings checked against the revised task text.
- Architecture/contracts: checked service/event ownership, capacity delivery, writer authority, grouped identity, copied-route dual contexts, overlay claims, and handoff.
- Safety/failure: checked proof atomicity, unknown ownership, fd-family closure, teardown release, generation correlation, and re-claim ordering.
- Compliance/verification: checked the authoritative resource design, normative inventory, governing synchronization/Present/direct contracts, and M-2 activation contract.
- Excerpts used: **12/12**. Verified source ground was limited to copied scanout’s distinct renderer/sink Vulkan-device requirement ([source lines 1322–1338](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/vk/scanout.rs:1322), [lines 1415–1420](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/vk/scanout.rs:1415)).
- Unassessed rather than presumed sound: exhaustive callers, private field migrations, dependency signatures, and hardware-specific cleanup behavior.
- Deferred to implementation: exact Rust correctness, builds/tests, clippy, formatting, portability compilation, Vulkan execution, and DRM hardware validation.