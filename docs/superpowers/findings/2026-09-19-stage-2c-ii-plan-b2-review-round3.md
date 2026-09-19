## Verdict

**3 blocking, 0 major, 0 minor**

**Coverage: COMPLETE FOR DECLARED SCOPE**

**Target:** plan B2 revision 3 (`2dd48980`), with prior round 2.
**Reviewer:** `codex exec --sandbox read-only`, single pass. **Instrument:** `docs/superpowers/review/` @ `69c6d6e2`; `gpt-5.6-sol`; `xhigh`; `codex-cli 0.154.0`.
**Recorded usage:** 106,886 tokens (exit 0). 12/12 excerpts.

**Author verification (2026-09-19):** all three CONFIRMED.
- B-1: `drain_page_flip_events` (`platform.rs` ~4850) calls `drain_owner_events` and discards every owner event except `LegacyPageFlip`, `LegacyClockSample` and `ClockSample` in `_ => {}` (~4967). `on_page_flip_ready` gets only `(flips, sequences)`.
- B-2: spec §7 "Activation" said the conductor acts only in `Owner`. The spec was amended rather than the plan bent, and the user has approved decision 9's intent.
- B-3: as stated.

This is a design-review result only; it does not claim compilation, passing tests, or implementation approval.

## Incorporation audit

| Prior finding | Status | Assessment |
| --- | --- | --- |
| Round 1 B-1 — unsafe/missing per-event wakes | **APPLIED** | The plan suppresses inner wakes, closes receipts/restores ledgers before one batch-final wake, and covers completion and rejection ordering ([plan lines 42–46](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-19-phase-c0-stage-2c-ii-plan-b2-maintenance-conductor.md:42)). |
| Round 1 M-1 — inadequate one-home evidence | **APPLIED** | Confirmation moves desired → submitted, and Unknown must remove submitted while preserving any newer desired generation; Q3 and Q19 target both faults ([plan lines 78, 156–158, 182](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-19-phase-c0-stage-2c-ii-plan-b2-maintenance-conductor.md:78)). |
| Round 1 M-2 — inadequate bounded-progress evidence | **APPLIED** | The drop test continues to primary dispatch, and the continuous-collision case now uses an always-rejected gamma with another identity ([plan lines 84, 182](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-19-phase-c0-stage-2c-ii-plan-b2-maintenance-conductor.md:84)). |
| Round 2 B-2 — every producer needs the batch helper | **PARTIAL** | All producers are named abstractly, but the actual DRM interface consumes and discards completion events before `KmsBackend` can call the helper. See B-1. |
| Round 2 B-3 — lifecycle drainage separate from admission | **TRADED** | Post-close drainage is now required, but it contradicts both the plan’s active-only global constraint and revision 4’s activation rule. See B-2. |
| Round 2 B-4 — Unknown stops admission | **APPLIED** | `recovery_stopped` suppresses both `admission_wake` and the batch-final wake, while receipt and resource drainage continue ([plan lines 49, 177, 182](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-19-phase-c0-stage-2c-ii-plan-b2-maintenance-conductor.md:49)). |
| Round 2 M-3 — cursor barrier invalidated collision test | **APPLIED** | The continuous-collision producer is gamma, whose drop records failure without raising the tier-2 cursor barrier. |

## Findings

### Blocking

#### B-1 — DRM completion events still cannot reach the batch helper

`PlatformBackend::drain_owner_events` does produce events from both `apply_drm_event` and `report_stream_failure` ([platform.rs lines 4265–4308](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/platform.rs:4265)). But production page-flip handling passes those events into `drain_page_flip_events`, which handles clock/legacy variants and silently discards every other `OwnerEvent` in `_ => {}` ([platform.rs lines 4850–4968](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/platform.rs:4850)). `KmsBackend::on_page_flip_ready` receives only `(flipped, sequences)`, so it has no owner-event batch to route ([backend.rs lines 20523–20552](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/backend.rs:20523)).

Concrete failure: `apply_drm_event` completes the live commit and emits retirement plus terminal events; `drain_page_flip_events` drops them. The owner slot becomes free, but the receipt remains open, submitted maintenance stays stranded, and no batch-final wake occurs. This violates retirement-first admission and receipt disposition ([spec lines 307–330, 358–371](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-18-phase-c0-stage-2c-ii-admission-design.md:307)).

Smallest correction: Task 1 must include `platform.rs` and define how `drain_page_flip_events` returns non-presentation owner events grouped as one fd-drain batch. `on_page_flip_ready` must pass that batch exactly once to `route_owner_event_batch`. The named test must traverse this interface; “say why unreachable” is not valid because the path is demonstrably reachable.

#### B-2 — Post-close drainage contradicts the authoritative activation contract

Decision 9 routes receipt/resource outcomes whenever a conductor is installed and a receipt or live commit exists, regardless of transport state ([plan line 48](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-19-phase-c0-stage-2c-ii-plan-b2-maintenance-conductor.md:48)). Yet the plan also says every new path is inert unless installed with transport in `Owner` ([plan lines 22, 60](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-19-phase-c0-stage-2c-ii-plan-b2-maintenance-conductor.md:22)), matching the authoritative specification: “The conductor acts only” in `Owner` ([spec lines 387–390](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-18-phase-c0-stage-2c-ii-admission-design.md:387)).

After a confirmed commit closes the transport on a bound violation, following decision 9 violates revision 4; following the spec strands its receipt and submitted payload. The combined receipt-after-close behavior also lacks an assigned final test: Task 1 explicitly proves only the resource half before receipts exist ([plan line 103](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-19-phase-c0-stage-2c-ii-plan-b2-maintenance-conductor.md:103)).

Smallest correction: amend the authoritative spec so activation governs new admission while lifecycle disposition is explicitly permitted after closure; align the global constraint accordingly. Assign Task 4 to extend the close test through receipt closure and submitted-payload disposition.

#### B-3 — “Any terminal” batch wake includes forbidden `NeverDispatched`

The helper wakes after any terminal outcome when the slot is free ([plan lines 42–46, 94](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-19-phase-c0-stage-2c-ii-plan-b2-maintenance-conductor.md:42)), while also claiming to be the sole route for production owner events. A pre-IPC `send_on` refusal produces `Terminal(NeverDispatched)` and resource-return events, although it has no receipt ([plan line 178](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-19-phase-c0-stage-2c-ii-plan-b2-maintenance-conductor.md:178)).

If routed through the sole helper, the free slot and terminal predicate immediately wake admission and retry unchanged desired state. The spec explicitly requires aborting, stopping, and waiting for the next real wake—no immediate retry ([spec lines 318–327](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-18-phase-c0-stage-2c-ii-admission-design.md:318)). Bypassing the helper instead violates its exclusivity contract.

Smallest correction: define wake eligibility by exact post-dispatch outcomes, explicitly excluding `NeverDispatched`; specify how its resource events use the common routing path without a final wake. Add a pre-IPC-refusal test proving only one dispatch attempt until an independent wake.

### Major

None.

### Minor

None.

## Coverage and implementation checks

- **Incorporation:** all seven carried findings were audited.
- **Architecture/contracts:** enumerated host-call drains, both `service_owner_completions` consumers, the five internal completion producers, and all three `drain_owner_events` consumers. The active page-flip consumer is the uncovered producer.
- **Safety/ownership:** checked slot release versus wakes, receipt closure, post-close drainage, Unknown parking, collision/drop consequences, and `NeverDispatched`.
- **Spec/verification:** checked bounds, conductor ordering, receipt outcomes, activation, rejection progress, named mutations, and assigned build/format/clippy/three-target gates.

**Excerpts used: 12/12:** four specification and eight source excerpts. Exact Rust signatures, borrow behavior, fixture mechanics, route-handler internals already accepted by A2/B1, compilation, tests, mutations, portability, and hardware execution remain deferred to implementation.