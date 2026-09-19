## Verdict

**2 blocking, 2 major, 1 minor**

**Coverage: COMPLETE FOR DECLARED SCOPE**

**Target:** plan B1 revision 2 (`4562ad8c`), with prior round 1.
**Reviewer:** `codex exec --sandbox read-only`, single pass
**Instrument:** `docs/superpowers/review/` @ `69c6d6e2`;
model `gpt-5.6-sol`; reasoning effort `xhigh`; `codex-cli 0.154.0`.
**Recorded usage:** 68,511 tokens (exit 0). 10/12 excerpts.

**Author verification (2026-09-18):** all five CONFIRMED.
- B-1: C.0 lines 1529–1533 forbid a plane-only successor over stale/incompatible required maintenance; lines 1505–1507 limit tier 3's requirement to identities "that would otherwise win".
- B-2: A1's `confirm` (`token.rs`) consumes only `decision.admitted`.
- M-1: one composed intent per CRTC (A1 storage); the scenario was unreachable.
- M-2, m-1: as stated.

This is a design-review result, not evidence that the implementation compiles, passes tests, or is approved for execution.

## Incorporation audit

| Prior finding | Status | Audit result |
|---|---|---|
| B-1 — A2 could dispatch maintenance-carrying tier 6 | **APPLIED** | The conductor must abort before routing on `admitted` whenever `carried` is non-empty, a combined primary exists, or the decision uses a new tier/variant ([plan line 57](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-18-phase-c0-stage-2c-ii-plan-b1-maintenance-decider.md:57)). The tier-6 conductor scenario and P20 are present ([plan line 72](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-18-phase-c0-stage-2c-ii-plan-b1-maintenance-decider.md:72), [line 190](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-18-phase-c0-stage-2c-ii-plan-b1-maintenance-decider.md:190)). |
| B-2 — allowance frozen at first ageing | **PARTIAL** | The governing decision and P24 scenario correctly make the allowance grow ([plan line 28](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-18-phase-c0-stage-2c-ii-plan-b1-maintenance-decider.md:28), [line 226](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-18-phase-c0-stage-2c-ii-plan-b1-maintenance-decider.md:226)), but the public `bound_violation` contract still describes the frozen-at-ageing rule. See m-1. |
| M-1 — missing negative scenarios P19–P22 | **APPLIED** | Ordinary-wake tier 3, incompatible symmetric primary, extra compatible maintenance, and compatible-but-Waiting absorption cases are now named ([plan lines 71–74](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-18-phase-c0-stage-2c-ii-plan-b1-maintenance-decider.md:71), [lines 190–202](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-18-phase-c0-stage-2c-ii-plan-b1-maintenance-decider.md:190)). |
| M-2 — real-confirm counters and rejection persistence | **APPLIED** | Task 4 requires real `lock`/`confirm` evidence; P23–P25 and the post-drop first-rejection scenario are present ([plan lines 76–78](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-18-phase-c0-stage-2c-ii-plan-b1-maintenance-decider.md:76), [line 143](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-18-phase-c0-stage-2c-ii-plan-b1-maintenance-decider.md:143), [lines 222–226](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-18-phase-c0-stage-2c-ii-plan-b1-maintenance-decider.md:222)). |
| m-1 — multiple authoritative gates | **APPLIED** | One gate now owns the feature clippy runs, portability targets, and coordinator-owned hardware gate ([plan lines 232–248](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-18-phase-c0-stage-2c-ii-plan-b1-maintenance-decider.md:232)). |

## Findings

### Blocking

#### B-1 — Direct successors can fall through to tier 6 despite maintenance that forbids a plane-only successor

The plan makes tier 3 depend on **every aged identity**, whether ready or not, then merely says tier 3 does not apply when that condition fails ([plan line 40](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-18-phase-c0-stage-2c-ii-plan-b1-maintenance-decider.md:40)). Tier-6 primaries absorb only ready compatible maintenance ([plan line 41](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-18-phase-c0-stage-2c-ii-plan-b1-maintenance-decider.md:41)).

Concrete failure: an aged cursor on the successor’s CRTC is stale/incompatible and therefore `Waiting`. Tier 3 fails, tier 4 cannot select it, and the successor can fall through to tier 6 without the cursor. C.0 expressly forbids that plane-only successor ([C.0 lines 1529–1533](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:1529)); the stage specification requires the successor to stay queued or be terminalized ([spec lines 216–219](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-18-phase-c0-stage-2c-ii-admission-design.md:216)). Conversely, an unrelated aged-but-Waiting identity must not suppress tier 3 because it would not otherwise win ([C.0 lines 1505–1507](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:1505)).

Smallest correction: define tier 3 over ready aged identities that would otherwise win, and separately make a direct successor ineligible in tier 6 when required maintenance on its closure is stale/incompatible. Add one test for each branch.

#### B-2 — Confirmation does not require consumption of the symmetrically combined primary

`combined_primary` identifies a primary physically included with a tier-4/7 maintenance winner ([plan lines 165–170](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-18-phase-c0-stage-2c-ii-plan-b1-maintenance-decider.md:165)), but the confirmation invariant only requires consuming the admitted intent and carried maintenance ([plan line 186](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-18-phase-c0-stage-2c-ii-plan-b1-maintenance-decider.md:186)). Existing A1 confirmation consumes only `decision.admitted`; `primary_crtcs()` merely advances service accounting ([token.rs lines 49–57](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/owner/admission/token.rs:49)).

Thus a cursor can confirm with composed generation G, while G remains in its desired slot and is submitted again on the next wake. C.0 requires both generations to retire with the combined request ([C.0 lines 1551–1555](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:1551)).

Smallest correction: explicitly consume `combined_primary` during `confirm`, while `abort` preserves it. Add a real lock/confirm/abort test checking slot removal and round-robin advancement.

### Major

#### M-1 — The oldest-compatible-primary test requires an impossible state

The named test requires “two composed generations” simultaneously ready on one CRTC ([plan line 190](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-18-phase-c0-stage-2c-ii-plan-b1-maintenance-decider.md:190)). The storage contract permits only one latest-wins composed slot per CRTC ([spec lines 93–99](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-18-phase-c0-stage-2c-ii-admission-design.md:93), [lines 111–114](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-18-phase-c0-stage-2c-ii-admission-design.md:111)). The second generation replaces the first, so the test cannot exercise a choice between two primaries.

Use two legal primary shapes covering the CRTC—composed and grouped direct—and verify both ordinal orders.

#### M-2 — New maintenance and recovery tiers lack negative readiness evidence

The plan tests a ready recovery and a Waiting maintenance generation not being **absorbed**, but never requires that aged/non-aged Waiting maintenance cannot win tiers 4/7 or that a Waiting recovery cannot win tier 2. Ignoring readiness during tier selection can therefore survive P1–P25. This violates the global no-dispatch-before-readiness criterion ([spec lines 147–161](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-18-phase-c0-stage-2c-ii-admission-design.md:147), [line 472](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-18-phase-c0-stage-2c-ii-admission-design.md:472)).

Add explicit Waiting-only tier-2, tier-4, and tier-7 scenarios plus a mutation that removes each readiness guard.

### Minor

#### m-1 — `bound_violation` still documents the rejected frozen allowance

The API says the allowance is based on older aged identities “when it aged” ([plan lines 214–218](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-18-phase-c0-stage-2c-ii-plan-b1-maintenance-decider.md:214)), contradicting the growing allowance fixed at [plan line 28](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-18-phase-c0-stage-2c-ii-plan-b1-maintenance-decider.md:28). Replace it with “aged at any moment since X aged.”

## Coverage and implementation checks

- **Incorporation:** all five round-1 findings audited against task text.
- **Architecture/contracts:** checked decision production, A2’s unsupported boundary, A1 confirmation consumption, tier fallthrough, and authoritative state ownership.
- **Safety/ownership:** checked ticket lifetime, exact-once primary consumption, rejection persistence, ageing, abort/confirm boundaries, and starvation accounting.
- **Spec/verification:** checked specification §§2–7, 10.2, 10.4 and 11.1, plus governing C.0 §9.2.1.

Used **10/12 bounded spec/source excerpts**. Unassessed by instruction: B2’s payload store, receipt and terminal routing; producer conversion; real compatibility/group derivation; stages 3–4. Exact Rust signatures, exhaustive matches, compilation, mutations, clippy, portability targets, and hardware behavior remain deferred to implementation gates.