## Verdict

**3 blocking, 1 major, 0 minor**

**Coverage: COMPLETE FOR DECLARED SCOPE**

**Target:** plan B2 revision 2 (`4107f091`), with prior round 1.
**Reviewer:** `codex exec --sandbox read-only`, single pass. **Instrument:** `docs/superpowers/review/` @ `69c6d6e2`; `gpt-5.6-sol`; `xhigh`; `codex-cli 0.154.0`.
**Recorded usage:** 85,226 tokens (exit 0). 12/12 excerpts.

**Author verification (2026-09-19):** all four CONFIRMED.
- B-2: `PlatformBackend`'s DRM drain calls `owner.apply_drm_event` per record (`platform.rs` ~4300), and that can reach `try_complete`.
- B-3, B-4, M-3: as stated.

Fixed in revision 3 (design decisions 8–10, Q21–Q24).

This is a design-review result only; it does not claim compilation, passing tests, or implementation approval.

## Incorporation audit

| Prior finding | Status | Assessment |
| --- | --- | --- |
| B-1 — unsafe/missing per-event wakes | **TRADED** | Completion and rejection handling now has a batch-level wake contract and ordering tests. However, removing the shared retirement wake without covering the DRM-event producer leaves that completion path with no wake at all. |
| M-1 — inadequate one-home evidence | **APPLIED** | Q3 moved to the confirm test; Unknown now asserts absence from submitted and has Q19. The newer-desired collision is also named. |
| M-2 — inadequate bounded-progress evidence | **PARTIAL** | The gamma-drop test now continues to primary dispatch. The two-identity scenario was added, but its always-rejected cursor installs a tier-2 recovery barrier that this plan deliberately cannot dispatch, so it cannot establish the claimed continuous-collision progress bound. |

## Findings

### Blocking

#### B-2 — DRM completions fall outside the new batch contract and lose their wake

The plan defines batches only for `apply_host_call_event` and `service_owner_completions` output, and removes the wake from `CompletionRetired` globally ([plan lines 34–38](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-19-phase-c0-stage-2c-ii-plan-b2-maintenance-conductor.md:34), [Task 1](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-19-phase-c0-stage-2c-ii-plan-b2-maintenance-conductor.md:80)).

There is another production owner-event producer: the DRM-event drain invokes `owner.apply_drm_event` and returns its events separately immediately before `service_owner_completions` ([platform.rs](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/platform.rs:4299)). `apply_drm_event` can extend its result with `try_complete`, whose accepted ordering is `CompletionRetired` followed by `Terminal(Completed)` ([device.rs](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/owner/device.rs:730), [device.rs](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/owner/device.rs:970)).

Concrete failure: a hardware event completes the live commit; both events are routed, the receipt closes and the slot becomes free, but the retirement arm no longer wakes and this producer has no specified batch-final wake. A queued intent stalls, violating immediate-on-retirement admission ([spec lines 307–330](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-18-phase-c0-stage-2c-ii-admission-design.md:307)).

Smallest correction: make one batch-routing helper authoritative for every production `OwnerEvent` producer, including the DRM drain, and add a DRM-completion trace test proving `Consumed → Enqueued → terminal handling → Decided → Dispatched` with one final wake.

#### B-3 — Active-only routing strands a live receipt after the bound closes transport

The plan creates the receipt and moves payloads to submitted after confirmation, then may immediately close the transport for `bound_violation()` ([plan lines 140–142](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-19-phase-c0-stage-2c-ii-plan-b2-maintenance-conductor.md:140)). Yet all new handling is declared inert unless the transport remains `Owner`, and host-call outcomes are routed only for an active conductor ([plan line 50](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-19-phase-c0-stage-2c-ii-plan-b2-maintenance-conductor.md:50), [plan line 80](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-19-phase-c0-stage-2c-ii-plan-b2-maintenance-conductor.md:80)).

Concrete failure: dispatch confirms maintenance; a receipt exists; the bound check closes the gate; the already-issued host call later rejects or becomes unknown. Because the conductor is no longer active, that event follows the log-and-drop path exemplified by current `record_host_call_events` ([backend.rs](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/backend.rs:19294)). The receipt never closes and its payload remains submitted, violating the receipt and one-home contracts ([spec lines 347–371](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-18-phase-c0-stage-2c-ii-admission-design.md:347)).

Smallest correction: separate eligibility for new admission from lifecycle drainage. Once a live commit or receipt exists, its owner events must continue through resource and receipt disposition after gate closure, while all new wakes remain suppressed. Test bound closure followed by a terminal outcome.

#### B-4 — `CompletionUnknown` wakes new admission instead of stopping it

Every terminal batch currently gets a final wake when the slot is free ([plan line 36](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-19-phase-c0-stage-2c-ii-plan-b2-maintenance-conductor.md:36)). Unknown handling merely parks payloads and closes the receipt ([plan lines 161–166](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-19-phase-c0-stage-2c-ii-plan-b2-maintenance-conductor.md:161)); it does not stop the conductor. `admission_wake` admits whenever the conductor remains active and the slot is free ([admission.rs](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/admission.rs:452)).

Thus `Terminal(CompletionUnknown)` can immediately dispatch another ready intent. The spec instead requires Unknown to stop admission and hand state to recovery ([spec line 371](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-18-phase-c0-stage-2c-ii-admission-design.md:371)). Deferring recovery re-offering does not defer this stop condition.

Smallest correction: mark admission recovery-stopped or close its gate before batch-final wake eligibility is evaluated. Extend the Unknown test with queued work and assert no subsequent dispatch.

### Major

#### M-3 — The continuous-collision test is blocked by its own cursor-recovery barrier

The scenario continually rejects a cursor and expects gamma on another CRTC to progress ([plan line 166](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-19-phase-c0-stage-2c-ii-plan-b2-maintenance-conductor.md:166)). On the cursor’s second rejection, however, the plan installs cursor recovery ([plan line 160](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-19-phase-c0-stage-2c-ii-plan-b2-maintenance-conductor.md:160)), while Task 3 leaves recovery persistently `Unsupported` ([plan line 136](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-19-phase-c0-stage-2c-ii-plan-b2-maintenance-conductor.md:136)). Tier 2 recovery then outranks gamma.

If the cursor owns the older ticket—the meaningful starvation case—it retries, drops, and leaves gamma blocked behind unsupported recovery. If gamma runs first, the test does not prove the intended bound.

Smallest correction: use a continuously rejected gamma identity for this two-identity progress test, since gamma drop creates no tier-2 barrier. Keep cursor-drop verification limited to proving recovery is raised.

### Minor

None.

## Coverage and implementation checks

- **Incorporation:** all three prior findings audited against revision 2.
- **Architecture/contracts:** checked all named batch producers and found the omitted DRM producer; checked receipt ownership, wake placement, and post-close drainage.
- **Safety/ownership:** checked completed, rejected, Unknown, collision, drop, gate-close, and one-home sequences.
- **Spec/verification:** checked §§5–7, 10.1–10.3 and 11.1. Build, nightly fmt, regular clippy, feature gates, repeated tests, release tests, and three-target checks are assigned to implementation.

**Excerpts used: 12/12:** five spec excerpts and seven source excerpts. Detailed owner/resource-consumer internals beyond the accepted event ordering, Rust signatures, borrow behavior, fixtures, compilation, test execution, mutation execution, and portability results remain deferred to implementation.