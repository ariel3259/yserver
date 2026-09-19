## Verdict

**1 blocking, 2 major, 0 minor**

**Coverage: COMPLETE FOR DECLARED SCOPE**

**Target:** plan B2 revision 1. **Reviewer:** `codex exec --sandbox read-only`, single pass.
**Instrument:** `docs/superpowers/review/` @ `69c6d6e2`; model `gpt-5.6-sol`; reasoning effort `xhigh`; `codex-cli 0.154.0`.
**Recorded usage:** 55,873 tokens (exit 0). 12/12 excerpts.

**Author verification (2026-09-19):** all three CONFIRMED.
- B-1: `try_complete` returns `CompletionRetired` then `Terminal(Completed)` (`device.rs` ~969). The Rejected arm returns `ResourcesReleased`, then `retire_live`'s `Terminal` and `ResourcesReleased`/`ResourcesStillCurrent` (~2464, ~2327). A2's retirement arm wakes admission inside the arm. Fixed by design decision 8, batch handling.
- M-1, M-2: as stated. Fixed with confirm-path and one-home evidence, the post-drop dispatch, and the two-identity scenario (Q17–Q20).

This is a design-review result only; it does not claim compilation, passing tests, or implementation approval.

## Incorporation audit

| Prior finding | Status | Assessment |
| --- | --- | --- |
| None | N/A | No prior review was supplied; check 1 was skipped as directed. |

## Findings

### Blocking

#### B-1 — Per-event routing wakes at the wrong lifecycle point and provides no safe rejection wake

The plan requires each returned owner event to pass individually through `route_owner_event` and leaves the retirement hook otherwise unchanged ([plan routing invariant](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-19-phase-c0-stage-2c-ii-plan-b2-maintenance-conductor.md:69), [Task 4](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-19-phase-c0-stage-2c-ii-plan-b2-maintenance-conductor.md:145)). That is incompatible with the owner’s actual event ordering:

- Successful completion emits `CompletionRetired` **before** `Terminal(Completed)` ([device.rs](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/owner/device.rs:969)). The current retirement arm consumes resources and immediately calls `admission_wake` ([backend.rs](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/backend.rs:19700)). A new commit can therefore be confirmed while the previous receipt is still open and its carried maintenance is still `submitted`. This violates the plan/spec’s one-receipt rule and lets the next snapshot observe stale maintenance current state ([spec receipt contract](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-18-phase-c0-stage-2c-ii-admission-design.md:358)).
- Kernel rejection emits an initial `ResourcesReleased`, then `Terminal`, then `ResourcesStillCurrent` through `retire_live` ([device rejection](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/owner/device.rs:2464), [retire_live](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/owner/device.rs:2325)). No `CompletionRetired` occurs, so the plan specifies no wake after the slot becomes free. Waking directly from `Terminal` would also be wrong because the following `ResourcesStillCurrent` has not yet restored the primary ledger.

The result is either a stalled rejected intent, a dispatch built over unrestored resources, or two live receipts. It also violates the required post-drop bounded progress ([spec §11.1](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-18-phase-c0-stage-2c-ii-admission-design.md:584)).

Smallest correction: define a batch-level owner-event contract. Route all events in order while suppressing admission wakes; after the complete owner-produced batch has closed the old receipt and restored resource state, issue exactly one eligible wake. Preserve the existing no-retry rule for pre-IPC refusals. Add completed and rejected batch-order tests with another ready intent.

### Major

#### M-1 — The one-home test cannot catch its assigned confirm mutation

The exit table assigns Q3, “leave the payload in desired after confirm,” to `c0_adm_conductor_offer_maintenance_fills_the_store_and_the_decider` ([plan exit table](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-19-phase-c0-stage-2c-ii-plan-b2-maintenance-conductor.md:53)). That test is introduced in Task 2, while confirmed maintenance dispatch and the desired-to-submitted move do not exist until Task 3 ([plan confirm transition](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-19-phase-c0-stage-2c-ii-plan-b2-maintenance-conductor.md:127)). Q3 can therefore survive its named evidence.

Unknown handling has the same proof gap: the plan adds a dormant store location ([plan unknown outcome](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-19-phase-c0-stage-2c-ii-plan-b2-maintenance-conductor.md:148)), but Q14/Q15 test re-offering and rejection counting, not whether the submitted entry was actually removed. A clone left both submitted and dormant would violate the spec’s exactly-one-home rule ([spec store](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-18-phase-c0-stage-2c-ii-admission-design.md:347)) while all listed mutations could pass.

Required correction: assign Q3 to a confirmed-dispatch test that asserts desired absent/submitted present, and add an unknown mutation that leaves submitted populated while parking dormant. Include the collision case where a newer desired generation already exists.

#### M-2 — Rejection tests do not establish the specification’s bounded-progress criteria

The spec requires both:

- after the second rejection/drop, affected primary work is admitted within the bound; and
- two competing identities progress under continuous newer-generation collision ([spec exit criteria](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-18-phase-c0-stage-2c-ii-admission-design.md:603)).

The plan’s tests establish re-entry, which payload survives one collision, cursor recovery, and gamma failure recording ([plan tests](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-19-phase-c0-stage-2c-ii-plan-b2-maintenance-conductor.md:153)). None requires primary admission after a drop or continuous collision between two identities. Q11–Q13 likewise do not kill a conductor integration that preserves the newer payload but resets its inherited rejection history or repeatedly lets it overtake the other identity.

Required correction: extend the gamma-drop scenario through the next primary dispatch, and add a conductor-level two-identity scenario that injects a newer generation during each rejected submission and asserts the `1 + 2(N - 1)` bound.

### Minor

None.

## Coverage and implementation checks

- **Incorporation:** no prior review.
- **Architecture/contracts:** checked host-call routing, owner-event order, receipt lifetime, resource restoration, maintenance store transitions, and wake placement.
- **Safety/ownership:** checked completed, rejected, collision, drop, and unknown sequences; identified the unsafe batch-boundary gap.
- **Spec/verification:** checked §§5, 7, and 11.1, named tests, Q1–Q16, and assigned build/clippy/format/three-target gates.

**Excerpts used: 12/12:** three spec excerpts; decider `reenter`; three `route_owner_event` excerpts; `record_host_call_events`; `apply_host_call_event_at`; rejection handling; `retire_live`; and `try_complete`.

Accepted A1/A2/B1 internals were not generally re-audited. Exact Rust signatures, borrow behavior, fixture construction, compilation, test execution, and portability results remain deferred to the real compiler and implementation gates.