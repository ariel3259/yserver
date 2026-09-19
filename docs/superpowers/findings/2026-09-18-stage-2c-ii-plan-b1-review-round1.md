## Verdict

**2 blocking, 2 major, 1 minor**

**Coverage: COMPLETE FOR DECLARED SCOPE**

**Target:** plan B1 revision 1 (`56641180`).
**Reviewer:** `codex exec --sandbox read-only`, single pass
**Instrument:** `docs/superpowers/review/` @ `69c6d6e2`;
model `gpt-5.6-sol`; reasoning effort `xhigh`; `codex-cli 0.154.0`.
**Recorded usage:** 68,723 tokens (exit 0). 12/12 excerpts.

**Author verification (2026-09-18):** all five CONFIRMED.
- B-1: A2's `admission_wake` routes solely on `decision.admitted` (`kms/render/admission.rs` ~471).
- B-2: the reviewer's sequence is legal under revision 1's rules and trips a frozen allowance.
- M-1, M-2, m-1: the named gaps are real; each got a scenario and a mutation (P19–P25) or a single gate.

## Incorporation audit

| Prior finding | Status |
|---|---|
| None | First review; check 1 skipped as instructed. |

## Findings

### Blocking

#### B-1 — A2 can dispatch a tier-6 primary while silently losing its carried maintenance

The plan requires tier-6 primaries to absorb compatible maintenance ([plan lines 33–36, 191–193](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-18-phase-c0-stage-2c-ii-plan-b1-maintenance-decider.md:33)), but its A2 compatibility rule aborts only new `Admitted` variants and tiers 3/4/5/7 ([plan lines 49–50](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-18-phase-c0-stage-2c-ii-plan-b1-maintenance-decider.md:49)). Tier 6 is omitted.

A2 currently dispatches solely by `decision.admitted`: an old `Admitted::Composed` or `Admitted::Direct` proceeds to its existing dispatcher ([source lines 464–481](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/admission.rs:464)). Therefore:

1. Tier 6 selects a composed primary carrying gamma.
2. A2 sees `Admitted::Composed` and dispatches only the primary.
3. Successful send confirms the whole decision; B1 consumes the gamma ticket and marks it submitted.
4. No B2 store, payload, receipt, or terminal routing exists, so that maintenance is absent from the request and cannot be completed or re-entered.

This violates the send-boundary ownership contract ([spec lines 269–297](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-18-phase-c0-stage-2c-ii-admission-design.md:269)) and the conductor/store contract ([spec lines 347–370](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-18-phase-c0-stage-2c-ii-admission-design.md:347)).

Smallest correction: before matching `Admitted`, A2 must abort any decision with non-empty `carried`, plus every new tier/variant, and return `Unsupported`. Add a conductor test proving tier-6-with-maintenance performs no source dispatch and leaves all admission state unchanged.

#### B-2 — Freezing the starvation allowance when an identity first ages produces false invariant failures

The plan records `2 × (older aged identities when it aged)` ([plan lines 21, 203–213](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-18-phase-c0-stage-2c-ii-plan-b1-maintenance-decider.md:21)). C.0 instead defines the allowance for the set of `N` incompatible identities that are aged, allowing `2(N−1)` older-ticket admissions ([C.0 lines 1579–1612](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:1579); [spec lines 252–267](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-18-phase-c0-stage-2c-ii-admission-design.md:252)). It does not freeze membership at the younger identity’s ageing transition.

Concrete legal sequence:

1. A has the older ticket but is `Waiting` and non-aged.
2. B ages behind an in-flight commit; the plan records zero older aged identities.
3. A becomes ready, then a barrier ages it without resetting its ticket.
4. After the barrier, A is admitted before B because its ticket is older.
5. B’s counter becomes one against a frozen allowance of zero, so `bound_violation()` fires, although `N=2` now permits two older-ticket admissions.

B2 would close the transport on a spec-compliant schedule.

Smallest correction: cohort accounting must expand when an older-ticket identity later becomes aged. Preserve that added allowance until the younger identity is carried or dropped; do not derive the budget solely from the original ageing instant.

### Major

#### M-1 — The named tests do not establish several advertised eligibility and absorption contracts

The verification table overclaims its mutations ([plan lines 64–67](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-18-phase-c0-stage-2c-ii-plan-b1-maintenance-decider.md:64)):

- Both tier-3 scenarios use a retirement wake, so an implementation that also permits tier 3 on ordinary wakes survives. C.0 makes tier 3 the retirement-time promotion path ([C.0 lines 1492–1527](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:1492)).
- P8 says “combine an incompatible primary,” but both symmetric-absorption scenarios explicitly provide a compatible primary ([plan line 181](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-18-phase-c0-stage-2c-ii-plan-b1-maintenance-decider.md:181)). P8 would survive.
- No symmetric scenario proves that other ready compatible maintenance on the combined primary’s CRTC is carried.
- No primary-absorption scenario supplies compatible-but-`Waiting` maintenance, so readiness can be ignored while P10 still fails only incompatible absorption.

Add explicit negative scenarios and corresponding mutations for ordinary-wake tier 3, incompatible symmetric primary, extra compatible maintenance, and compatible-but-not-ready maintenance.

#### M-2 — Counter tests can pass while real confirmation accounting and post-drop persistence are broken

`c0_adm_maint_bound_violation_is_reported` may drive state through a direct test hook ([plan lines 213–215](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-18-phase-c0-stage-2c-ii-plan-b1-maintenance-decider.md:213)). That proves the final comparison, not that `confirm` increments younger identities or removes counters when carried/dropped. Deleting the real increment path can leave every named behavioral test passing.

Likewise, no test covers the required post-drop rule: the rejection count survives the drop, so the next generation’s first rejection drops immediately ([spec lines 596–600](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-18-phase-c0-stage-2c-ii-admission-design.md:596)). Resetting the count on drop survives the current tests.

Require transition-level evidence through `confirm`, including increment, carry/drop cleanup, and “new generation after drop is rejected once and immediately dropped.”

### Minor

#### m-1 — The authoritative gate is split across contradictory instructions

Task 4 says to run feature-specific clippy and three-target checks ([plan line 217](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-18-phase-c0-stage-2c-ii-plan-b1-maintenance-decider.md:217)), but the “Gate (every task)” block omits both ([plan lines 221–231](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-18-phase-c0-stage-2c-ii-plan-b1-maintenance-decider.md:221)), and the coordinator is told only to rerun “the gate.” The authoritative spec also assigns the three portability targets and final hardware gate ([spec lines 524–531](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-18-phase-c0-stage-2c-ii-admission-design.md:524)).

Make one gate authoritative, with explicit feature/target commands and ownership of the user-approved hardware run.

## Coverage and implementation checks

- **Incorporation:** skipped; no prior review.
- **Architecture/contracts:** checked plan tasks, B2-facing decision data, A1 decision/lock/confirm behavior, and A2 dispatch routing.
- **Safety/ownership:** checked ticket consumption, ageing, rejection persistence, payload/receipt ownership, send-boundary confirmation, and failure handoff.
- **Spec/verification:** checked C.0 §9.2.1, admission-spec §§3–7, 10.2, 10.4 and 11.1, plus every named B1 criterion/mutation.

Used **12/12 bounded excerpts**. Unassessed by design: B2 implementation, producer conversion, real compatibility/group derivation, and compiler-level API correctness. No builds, tests, mutations, compilation experiments, or review scripts were run; those remain implementation checks, not evidence of soundness here.