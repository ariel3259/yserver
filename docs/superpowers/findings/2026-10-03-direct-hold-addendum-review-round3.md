# Direct-hold Owner addendum — design review round 3

**Result:** 0 blocking, 2 major, 0 minor; coverage INCOMPLETE (24/24 excerpts).

**Reviewer:** `codex exec --sandbox read-only`, single pass
**Instrument:** `yserver-review` @ `111490f`;
model `gpt-6.1-sol`; reasoning effort `xhigh`; `codex-cli 0.159.2`.
Counts are comparable only to other reviews citing this same instrument SHA.

**Coordinator verification:** M-1 verified (first direct entry sets M2
`pending` with no current frame, backend.rs ~3850). M-2 verified (a client
modeset on the direct device requests `client_modeset_direct_ineligible` and
resumes after the unflip retires, admission.rs ~5340-5373). Both design.
Addressed in addendum revision 4; the invariant is restated negatively so
further unlisted states need no new enumeration.

## Verdict

0 blocking, 2 major, 0 minor

Coverage: INCOMPLETE

Revision 3 addresses successor resource retention and mutation masking, but its ownership invariant still excludes initial direct entry, and its lifecycle rule contradicts the existing client-modeset handoff. This is a design-review result; it does not establish compilation, test success, or implementation approval.

## Incorporation audit

| Prior finding | Disposition |
|---|---|
| Round 2 M-1 — Ledger ownership during direct replacement | **PARTIAL.** [Rule 4, lines 89–100](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-10-03-phase-c0-direct-hold-owner-addendum.md:89) correctly places D0 in the successor commit’s old-state dependencies and correlates D1’s new state. However, both permitted states require an existing M2-current D0, excluding initial direct entry. See M-1. |
| Round 2 M-2 — Pending and queued-successor mutation masking | **APPLIED.** [Lines 131–142](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-10-03-phase-c0-direct-hold-owner-addendum.md:131) make pending ownership a characterization, preserve the occupied-slot barrier, and require a waiting queued successor so it cannot mask composed dispatch. The pending test’s rule-4 assertion remains affected by M-1. |
| Carried round 1 M-1 — Protected CRTC set | **APPLIED.** Rule 0 defines the complete grouped set and requires identical use for readiness, bundles, absorption and assertions. Partial-device direct ownership remains refused. |
| Carried round 1 M-2 — Lifecycle exceptions | **PARTIAL.** Lifecycle exceptions and proof-based release remain explicit, but the blanket claim that client modesets avoid Ciii contradicts production behavior. See M-2. |
| Carried round 1 M-3 — Ready composed mutation precondition | **APPLIED.** Lines 114–122 require a production-produced offer, independent readiness and dispatch-boundary evidence; the pending and queued qualifications address the prior masking objection. |

## Findings

### Blocking

None demonstrated within the inspected scope.

### Major

**M-1 — Rule 4 excludes the first direct frame before retirement**

[Addendum lines 85–100](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-10-03-phase-c0-direct-hold-owner-addendum.md:85) require direct ownership to be either current D0 or current D0 plus dispatched successor D1. Initial direct entry satisfies neither state.

Concrete sequence: the device currently displays composed content; its first direct candidate is queued and dispatched. Dispatch preparation explicitly supports composed current resources without a `DirectRole::Current` allocation ([backend.rs lines 24910–24935](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/backend.rs:24910)). Dispatch confirmation installs the new frame in M2 `pending`, without creating an M2 current predecessor ([lines 3834–3852](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/backend.rs:3834)). Promotion into M2 `current` happens at retirement ([lines 3895–3920](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/backend.rs:3895)).

An iteration between dispatch and retirement therefore has pending direct ownership but no D0, without any excluded lifecycle, modeset or unflip transition. The mandated per-iteration observation would reject valid direct entry. The pending test also incorrectly labels this interval “Direct replacement in flight” ([addendum lines 131–137](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-10-03-phase-c0-direct-hold-owner-addendum.md:131)).

The authoritative spec requires preservation of direct entry and its Owner transaction machinery ([spec lines 2038–2058](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:2038)).

**Smallest correction:** add explicit initial-entry ownership states, including queued-only ownership and dispatched direct ownership without a direct predecessor. Correlate the pending direct frame with its exact new ledger resources and retained prior composed state. Exercise first direct entry through production entries with the per-iteration assertion.

**M-2 — Rule 2 incorrectly exempts client modesets from their Ciii prerequisite**

[Addendum lines 72–81](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-10-03-phase-c0-direct-hold-owner-addendum.md:72) say the listed lifecycle/topology paths, including a client modeset, “are not routed through Ciii,” while also requiring their existing handoffs to remain unchanged. Those instructions conflict.

Concrete sequence: D0 covers CRTCs A and B; a client requests a modeset on that device. The existing predicate requires direct unflip whenever the modeset device owns the active direct frame ([admission.rs lines 5208–5284](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/admission.rs:5208)). The parking path removes the modeset’s topology admission, requests direct unflip and waits for its completion ([lines 5340–5373](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/admission.rs:5340)); resumption is correlated with the unflip commit’s retirement ([lines 5400–5424](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/admission.rs:5400)).

Following the blanket exemption would bypass or mischaracterize this grouped replacement prerequisite. That matters because direct-slot eligibility requires replacement of the complete authoritative output set, and topology changes require Owner quiescence ([spec lines 1110–1115](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:1110), [1049–1051](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:1049)).

**Smallest correction:** distinguish bypassing the ordinary composed gate from bypassing Ciii. Explicitly preserve the client-modeset prerequisite unflip and commit-correlated resumption. Add a production-entry witness with grouped direct ownership, a parked client modeset, failed shadow materialization, retry and unflip retirement; assert that modeset dispatch follows that retirement.

### Minor

None.

## Coverage and implementation checks

- **1 — Incorporation:** assessed both round-2 findings and the three carried findings. Successor retention and mutation masking corrections are present; the invariant and lifecycle contract remain incomplete.
- **2 — Architecture/contracts:** inspected grouped eligibility, readiness production, candidate filtering, direct dispatch, topology dispatch separation and client-modeset parking. Partial-device direct ownership remains explicitly refused. No new rule-1 admission deadlock was demonstrated.
- **3 — Safety/ownership/failure:** verified direct old-state retention and pending-to-current promotion. Unflip admission is queued before shadow materialization, and its existing CRTC barrier blocks ordinary primaries independently of shadow readiness. DPMS descriptions retain primary-plane bindings; acquire clears direct state through completed reinstall promotion. Complete VT/quarantine disposition and maintenance retirement were not established.
- **4 — Specification/verification:** assessed production-offer preconditions, pending characterization, waiting successors, grouped protection, retry and progress witnesses. A/B/F and coordinator hardware execution are assigned. DPMS/acquire ordering evidence is not specified explicitly; it is not established by the maintenance/VT-release progress witness.

**Excerpts used: 24/24**, excluding the single reads of the target and prior review and bounded locator searches. Investigation stopped at the limit.

The remaining bounded question is whether complete VT/quarantine and primary-free maintenance retirement preserve grouped direct leases until actual replacement or teardown proof. That ground is **unassessed, not sound**; the spec requires accepted-buffer release to wait for replacement or teardown evidence ([lines 1921–1928](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:1921)).

No builds, tests, installs or compilation experiments ran. Implementation must perform `cargo +nightly fmt`, `cargo clippy --all-targets -- -D warnings`, applicable build/portability checks, existing core-driver and end-state checks, suites and mutations under CPU load, and coordinator hardware validation.