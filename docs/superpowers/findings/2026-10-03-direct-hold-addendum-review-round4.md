# Direct-hold Owner addendum — design review round 4

**Result:** 0 blocking, 2 major, 0 minor; coverage INCOMPLETE (24/24 excerpts).

**Reviewer:** `codex exec --sandbox read-only`, single pass
**Instrument:** `yserver-review` @ `111490f`;
model `gpt-6.1-sol`; reasoning effort `xhigh`; `codex-cli 0.159.2`.
Counts are comparable only to other reviews citing this same instrument SHA.

**Coordinator classification:** every design rule is APPLIED (protected set,
rule 1, lifecycle and client-modeset paths) and no admission deadlock was
found. M-1 is the invariant's wording at a boundary state (pre-entry composed
content while the first direct frame is queued) and M-2 a test-mutation
reachability detail: edge cases under the user's convergence rule
(2026-09-28 / 2026-10-03: rounds continue only while they find design
defects). Both folded into addendum revision 5 without another round.

## Verdict

0 blocking, 2 major, 0 minor

Coverage: INCOMPLETE

The client-modeset correction preserves its Ciii prerequisite. However, the revised invariant still rejects valid initial-entry ownership, and the new first-entry dispatch mutation is masked by the existing occupied-slot barrier. This result does not establish compilation, test success, or implementation approval.

## Incorporation audit

| Prior finding | Disposition |
|---|---|
| Round 3 M-1 — Initial direct-entry ownership | **TRADED.** Rule 4(b) now permits pending direct ownership without a direct predecessor, retaining prior composed resources. Rule 4(a), however, forbids the composed current state that can legitimately coexist with queued-only initial entry. See M-1. |
| Round 3 M-2 — Client-modeset prerequisite | **APPLIED.** [Plan lines 78–93](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-10-03-phase-c0-direct-hold-owner-addendum.md:78) explicitly preserve parking, prerequisite unflip, retirement-correlated resumption and proof-based release; lines 179–183 add the requested witness. |
| Carried round 2 M-1 — Successor ledger ownership | **APPLIED.** [Rule 4(b)](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-10-03-phase-c0-direct-hold-owner-addendum.md:105) distinguishes dispatched new direct resources from retained predecessor resources, including an initial composed predecessor. |
| Carried round 2 M-2 — Mutation masking | **TRADED.** Lines 146–157 correctly characterize pending ownership and prevent a ready successor from masking composed admission. Lines 174–178 reintroduce an unreachable dispatch mutation during pending initial entry. See M-2. |
| Carried round 1 M-1 — Protected CRTC set | **APPLIED.** Rule 0 defines complete grouped coverage consistently across readiness, bundles, absorption and assertions; partial-device direct ownership remains refused. |
| Carried round 1 M-2 — Lifecycle handoffs | **APPLIED at the design-contract level.** Rule 2 separates prerequisite Ciii paths from lifecycle paths and prohibits early release. Complete teardown implementation evidence remains unassessed below. |
| Carried round 1 M-3 — Ready composed precondition | **APPLIED.** Lines 129–137 require a production-produced offer, independent readiness and dispatch-boundary evidence. This does not overcome M-2’s occupied-slot barrier. |

## Findings

### Blocking

None demonstrated.

### Major

**M-1 — Rule 4(a) still excludes valid queued-only initial entry**

[Plan lines 97–115](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-10-03-phase-c0-direct-hold-owner-addendum.md:97) prohibit any composed primary in flight or in `current_resources` whenever M2 owns a queued, pending or current direct frame. Line 113 expressly applies this prohibition to queued-only ownership.

Concrete sequence: composed content C0 is current; the first direct frame D1 is prepared and queued; D1 waits for producer readiness or an already occupied device slot. Queuing installs M2’s successor and direct hold without replacing C0’s current resources ([backend.rs lines 24888–24895](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/backend.rs:24888)). The production offer records the successor separately from dispatch ([admission.rs lines 8060–8079](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/admission.rs:8060)); admission can subsequently return `SlotBusy` ([lines 8508–8517](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/admission.rs:8508)).

At that iteration boundary, M2 owns queued D1 while C0 legitimately remains current. Rule 4(a) fails despite no composed commit being admitted over the hold. The same problem applies to a composed commit accepted before D1 was queued. The authoritative spec preserves bounded submitted/successor state and direct-entry machinery ([spec lines 1089–1099](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:1089), [2038–2054](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:2038)).

**Smallest correction:** distinguish pre-existing composed ownership from newly admitted composed replacement. Permit the exact pre-entry composed current/in-flight state while initial direct ownership is queued; prohibit subsequent ordinary composed dispatch onto the protected set. Preserve that predecessor through its existing replacement proof. Exercise queued initial entry with a genuine wait across an iteration boundary.

**M-2 — The first-entry dispatch mutation cannot reach readiness**

The new test requires admitting a composed primary while the first direct frame is pending ([plan lines 174–178](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-10-03-phase-c0-direct-hold-owner-addendum.md:174)). But the plan already correctly states that pending direct ownership occupies the device slot and that the slot must never be bypassed to manufacture a witness ([lines 146–153](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-10-03-phase-c0-direct-hold-owner-addendum.md:146)).

Concrete sequence: D1 dispatches and becomes M2-pending; composed offer C1 is independently ready; rule 1 is removed. `admission_wake` still returns `SlotBusy` before obtaining a readiness snapshot ([admission.rs lines 8508–8519](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/admission.rs:8508)). C1 cannot dispatch, so the proposed gate mutation cannot establish the claimed sensitivity. Removing the slot barrier instead would test a different contract and contradict the plan’s own prohibition. The spec requires topology/ownership work not to overtake submitting or accepted commits ([spec lines 1143–1145](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:1143)).

**Smallest correction:** make pending first entry an ownership characterization, optionally with a production bookkeeping mutation that breaks exact commit/resource correlation. Use the existing post-retirement ready-offer witness for the composed-admission mutation; retain the occupied-slot barrier.

### Minor

None.

## Coverage and implementation checks

1. **Incorporation:** assessed both round-3 findings and all five carried findings. Initial pending correlation and client-modeset parking are incorporated; the invariant and mutation claims remain overstated as described above.

2. **Architecture and contracts:** verified grouped homogeneous single-device eligibility, complete-device successor descriptors, readiness construction, occupied-slot ordering, unflip candidate exclusion and separate lifecycle dispatch. Multi-device and partial-device direct topologies remain refused by the production predicate ([backend.rs lines 4754–4785](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/backend.rs:4754)). No rule-1 deadlock was demonstrated in the inspected ordinary admission or unflip retry paths.

3. **Safety, ownership and failure:** verified queued-to-pending ownership, exact recorded-unflip retirement, maintenance-only retirement routing, DPMS descriptions retaining primary bindings, and acquire promotion calling direct cleanup. The production tick retries failed shadow materialization and wakes unflip admission ([backend.rs lines 28621–28637](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/backend.rs:28621)). Existing unflip barriers independently exclude ordinary composed candidates while shadow readiness waits.

4. **Specification and verification:** assessed production-ready offer preconditions, pending characterization, waiting successors, grouped protection, retry and progress witnesses. A/B/F and coordinator hardware execution are assigned. The maintenance/VT-release progress witness does not establish DPMS-cycle or acquire-reinstall ownership ordering.

**Excerpts used: 24/24**, excluding the single reads of the target and prior review and locator searches. Investigation stopped at the boundary.

**Unassessed, not sound:** the complete maintenance request resource closure; VT/removal/quarantine disposition of direct leases and source pins; and the completion proof preceding acquire cleanup. A bounded follow-up would resolve whether those paths retain grouped direct ownership until actual replacement or complete teardown proof, as required by [spec lines 1627–1637](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:1627) and [1921–1928](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:1921).

No builds, tests, installs or compilation experiments ran. Implementation retains the real compiler/build and portability checks, `cargo +nightly fmt`, `cargo clippy --all-targets -- -D warnings`, core-driver/end-state checks, suites and mutations under CPU load, and coordinator hardware validation.