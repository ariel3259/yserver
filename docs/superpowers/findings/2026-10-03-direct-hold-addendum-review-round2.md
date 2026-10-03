# Direct-hold Owner addendum — design review round 2

**Result:** 0 blocking, 2 major, 0 minor; coverage INCOMPLETE (24/24 excerpts).

**Reviewer:** `codex exec --sandbox read-only`, single pass
**Instrument:** `yserver-review` @ `111490f`;
model `gpt-6.1-sol`; reasoning effort `xhigh`; `codex-cli 0.159.2`.
Counts are comparable only to other reviews citing this same instrument SHA.

**Coordinator verification:** M-1 verified (direct dispatch takes D0 out of
`current_resources` into the new commit's dependencies, admission.rs ~9330,
while M2 keeps D0 current until D1 retires). M-2 verified (`admission_wake`
returns `SlotBusy` before the snapshot, admission.rs ~8510). Classification:
M-1 design (the invariant is a normative contract); M-2 test design.
Addressed in addendum revision 3.

## Verdict

0 blocking, 2 major, 0 minor

Coverage: INCOMPLETE

Revision 2 incorporates the protected-set and lifecycle corrections, but its invariant rejects a legitimate direct-successor transition, and its mutation preconditions still allow masking by existing admission barriers. This is a design-review result; it does not establish compilation, test success, or implementation approval.

## Incorporation audit

| Prior finding | Disposition |
|---|---|
| M-1 — Authoritative protected CRTC set | **APPLIED.** Rules 0–1 define the complete device group, distinguish it from the Present pacing CRTC, cover pending/current/queued ownership, and require consistent use for readiness, bundles and absorption. Partial-device ownership remains refused ([addendum lines 52–70](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-10-03-phase-c0-direct-hold-owner-addendum.md:52)). |
| M-2 — Lifecycle and quarantine exceptions | **APPLIED.** Rules 2 and 4 qualify the ordinary unflip-only rule, preserve lifecycle/modeset handoffs, and reject logical withdrawal as replacement proof ([lines 71–90](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-10-03-phase-c0-direct-hold-owner-addendum.md:71)). This resolves the prior absolute-rule defect; it does not establish every existing handoff’s correctness. |
| M-3 — Ready composed precondition and mutation reachability | **PARTIAL.** Production-produced retained offers, independent producer readiness, requested variants, retry coverage and per-iteration observations are specified ([lines 102–135](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-10-03-phase-c0-direct-hold-owner-addendum.md:102)). Producer readiness alone still cannot establish mutation reachability in the pending and queued-successor cases; see M-2 below. |

## Findings

### Blocking

None demonstrated within the inspected scope.

### Major

**M-1 — Rule 4 mistakes ledger ownership during direct replacement for lost ownership**

Rule 4 requires M2’s current frame to remain in `current_resources` at every iteration boundary, except during lifecycle, modeset or Ciii transitions ([addendum lines 84–90](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-10-03-phase-c0-direct-hold-owner-addendum.md:84)). It omits an ordinary direct successor in flight.

Concrete sequence: D0 is M2-current; admission dispatches direct successor D1. Direct dispatch removes D0’s resource group from `current_resources` and registers it as the new commit’s old-state dependencies ([admission.rs lines 9320–9334](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/admission.rs:9320)). M2 keeps D0 current while D1 is pending; it replaces D0 only when D1 retires ([backend.rs lines 3848–3852 and 3905–3922](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/backend.rs:3848)). Thus the iteration can end with valid retained ownership but no matching entry in `current_resources`.

This conflicts with the required direct-successor admission path ([spec lines 1171–1205](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:1171)). The proposed per-iteration assertion would reject legitimate successor traffic; satisfying it by withholding successors would violate that scheduling contract.

**Smallest correction:** distinguish stable current ownership from a correlated direct replacement in flight. In the latter state, require D0’s retained old-state ownership and D1’s pending ownership in the commit ledger, with their exact identities and protected set. Preserve the stable-state `current_resources` check, so an ordinary composed replacement cannot hide behind a generic “commit in flight” exception.

**M-2 — Pending and queued-successor mutations can survive despite a ready composed offer**

The shared test contract requires removing the gate to dispatch a composed commit, then applies “the same” test to pending direct ownership and a queued successor ([addendum lines 102–120](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-10-03-phase-c0-direct-hold-owner-addendum.md:102)).

For a submitted direct frame that is not yet presented, the device slot is occupied. Production `admission_wake` returns `SlotBusy` **before constructing the readiness snapshot** ([admission.rs lines 8508–8519](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/admission.rs:8508)). Removing rule 1 therefore cannot dispatch the ready composed offer during that interval. Letting the direct commit retire makes the mutation observable in the *current* state, without establishing pending-state gate sensitivity.

The queued-successor variant has another mask: a ready direct successor can win retirement admission even after the composed gate is removed ([decide.rs lines 45–56](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/owner/admission/decide.rs:45)). This is required scheduling behavior ([spec lines 1184–1205](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:1184)), not a fixture defect that independent composed readiness eliminates.

**Smallest correction:** specify the distinguishing production observation for each variant. For queued ownership, establish a production-reachable waiting successor so it cannot mask composed dispatch. For pending ownership, either identify a reachable gate-sensitive observation or explicitly make it an ownership characterization, using the post-retirement case for dispatch mutation evidence. Do not bypass the occupied slot to manufacture the witness. Narrow the M-3 incorporation claim accordingly.

### Minor

None.

## Coverage and implementation checks

- **Check 1 — Incorporation:** assessed all three prior findings against revision 2. Two applied; mutation reachability remains partial.
- **Check 2 — Architecture/contracts:** inspected snapshot production, selection, absorption and dispatch. Filtering composed readiness also removes it from bundle and maintenance absorption candidates. A requested unflip blocks protected primaries independently of shadow readiness ([decide.rs lines 259–263](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/owner/admission/decide.rs:259)). No new admission deadlock was demonstrated.
- **Check 3 — Ownership/failure:** inspected current-resource extraction, direct promotion, unflip dependency capture, production retry and DPMS description. DPMS changes `ACTIVE` while retaining primary-plane bindings ([admission.rs lines 3081–3096](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/admission.rs:3081)); its completion cannot alone justify releasing a still-bound direct allocation.
- **Check 4 — Verification:** confirmed the existing unmap test uses real import/window-release entries and core-driver completion. New tests specify production entries and A/B/F discipline, but M-2 prevents their stated mutations from proving every claimed state. Hardware execution remains assigned to the coordinator.

**Excerpts used: 24/24**, excluding the target and prior-review reads and bounded locator searches. Investigation stopped at the budget.

Unassessed ground includes complete VT acquire/quarantine resource disposition, client-modeset handoffs under grouped direct ownership, pure-maintenance resource retention, and exhaustive unflip-trigger coverage. These are not established as sound. A bounded follow-up should answer whether exempt lifecycle and maintenance commits preserve grouped direct ownership until their actual replacement or teardown proof.

No builds, tests, installs or compilation experiments ran. Implementation must perform `cargo +nightly fmt`, `cargo clippy --all-targets -- -D warnings`, applicable portability checks, A/B checks, suites and mutations under CPU load, and coordinator hardware validation.