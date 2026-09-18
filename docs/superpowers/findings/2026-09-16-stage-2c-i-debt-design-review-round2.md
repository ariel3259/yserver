## Verdict

**0 blocking, 3 major, 0 minor**

**Coverage: COMPLETE FOR DECLARED SCOPE**

**Target:** `docs/superpowers/specs/2026-09-15-phase-c0-stage-2c-i-debt-design.md`,
revision 2 (`d1bbd5b3`), against the authoritative parent
`docs/superpowers/specs/2026-09-08-phase-c0-stage-2c-i-resource-terminalization-design.md`,
with `--prior` round 1. The reviewer was told the parent does not contain the
Owner handover contract and pointed at the 2c-i plan's Task 6 and the stage-2c
design's section 6.

**Reviewer:** `codex exec --sandbox read-only`, single pass
**Instrument:** `docs/superpowers/review/` @ `783089b4`;
model `gpt-5.6-sol`; reasoning effort `medium`; `codex-cli 0.153.4`.
Counts are comparable only to other reviews citing this same instrument SHA.
**Comparable to round 1** (same SHA): 3 blocking + 1 major → 0 blocking + 3 major.

**Recorded usage:** 50,815 tokens reported by the completed process (exit 0),
excluding the author session. 12/12 bounded excerpts. No build, test or nested
reviewer was run.

**Author verification (2026-09-16):**

- **M-1 — CONFIRMED from the author's own census data.** `apply_teardown_release`
  holds two family-E identity guards (the proof's incarnation and the key), and
  `validate_gpu_batch` holds three (one per check set). A tag naming family and
  function cannot tell them apart, so the association is not one-to-one; and a
  tagged test failing for an unrelated reason would still credit the guard.
- **M-2 — CONFIRMED as a coverage requirement, with a severity note.** The reset
  design's invariant 6 is verbatim: "No old-generation object, mapping or queued
  operation is ever interpreted as belonging to a newly reused numeric id." The
  revision-2 assertions never cross into the next generation or reuse the XID.
  However, the concrete failure the finding describes — a late old proof routed
  by numeric id to the new backing — is unlikely **at the ledger layer**:
  `AllocationKey` carries `device`, `incarnation` **and `generation`**
  (`availability.rs:7-11`), so proofs are not keyed by XID. The exposed layer is
  the store's XID-to-drawable mapping across the reset. The requirement stands,
  because "structurally protected" is an untested claim through this path.
- **M-3 — CONFIRMED.** The 2c-i plan's Task 6 requires that tests "construct it
  only after explicit mock/disabled coverage for every class", and the stage-2c
  design (lines 277–279) allows Owner publication "only when all writer classes
  are either owner-mediated or disabled and the teardown receiver is installed".
  Revision 2's section 6 reasoned only about production issuers. Family A's
  tests would have been built on an empty coverage proof and an identityless
  reservation, certifying a state the contract forbids.

The reviewer also confirmed a feasibility question the author raised:
`force_destroy_all_clients` takes the real `Backend` trait object and
`KmsBackend::free_pixmap` reaches the store decrement path, and a
fixture-installed managed allocation is not production Owner publication under
R8.

No finding was a false positive. Corrections pending the user's call on one
structural consequence of M-3; no third external pass launched.

This is a design-review result only. It does not claim compilation, test success, or implementation approval.

## Incorporation audit

| Prior finding | Status | Audit |
|---|---|---|
| B-1 — reset conflated logical erasure and physical destruction | **PARTIAL** | Section 4.3 correctly replaces the contradiction with logical XID erasure plus proof-gated backing retention ([target lines 188–220](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-15-phase-c0-stage-2c-i-debt-design.md:188)). Its proposed test, however, drives only forced client destruction and does not exercise the generation-replacement/reused-XID half of the reset contract. See M-2. |
| B-2 — receipt was neither compulsory nor identity-correlated | **APPLIED** | Section 4.1 withdraws `#[must_use]`, binds the token to device/incarnation, consumes it on unregister, and specifies fail-closed drop behavior ([target lines 145–168](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-15-phase-c0-stage-2c-i-debt-design.md:145)). This is achievable using the cited shared fail-closed latch pattern: an undischarged `RoleReservation::drop` closes admission ([capacity.rs lines 36–68](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/resources/capacity.rs:36)). |
| B-3 — debug underflow check could consume another husk’s count | **APPLIED** | The scalar/saturating decrement is replaced conceptually by exact-token consumption and unconditional fail-closed handling for foreign, unknown, or dropped registrations. |
| M-1 — zero survivors overstated systematic refusal coverage | **PARTIAL** | Revision 2 adds `mod.rs`, explicitly limits the census, derives families from contracts, and stops calling absent/non-guard refusals covered. The tag oracle still cannot establish the intended causal assertion. See M-1. |

## Findings

### Blocking

None.

### Major

#### M-1 — The tag-beside-test oracle does not identify or validate the intended killing assertion

The association tag names only a **family and function** ([target lines 74–85](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-15-phase-c0-stage-2c-i-debt-design.md:74)), while several listed functions contain multiple same-family guards: `apply_teardown_release` has key and proof-incarnation checks, and `validate_gpu_batch` has identity checks across three check sets. Consequently, the metadata does not provide a one-to-one guard/oracle association.

Concrete failure: a tagged broad test reaches `validate_gpu_batch` but fails on an earlier identity check, fixture panic, or common postcondition. Mutating a later guard still makes that tagged test fail, so the tool records the intended guard as caught without demonstrating the later refusal or its required disposition. Co-location prevents list drift but does not distinguish causal failure.

Required correction: give each enumerated mutation a stable site/invariant identifier and require the adjacent tag to name that identifier. The tool must also verify a guard-specific observable—expected error/disposition/state marker—or otherwise establish that the tagged assertion was reached. “A tagged test failed” alone must not count.

#### M-2 — The reset test stops before the generation boundary it claims to verify

Section 4.3 drives `force_destroy_all_clients` and asserts XID removal, retained backing, then proof-driven destruction ([target lines 188–220](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-15-phase-c0-stage-2c-i-debt-design.md:188)). Those assertions correctly express logical-versus-physical lifetime. The proposed mutation—prematurely destroying a backing with a pending obligation—is meaningful.

But `force_destroy_all_clients` is only the forced resource-destruction component; the actual boundary subsequently replaces session state. The reset specification separately requires that no old-generation object or queued operation be interpreted as a newly reused numeric ID ([reset spec lines 340–358](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-09-server-reset-design.md:340)). The current three assertions never create the next generation or reuse the numeric XID.

Concrete failure: forced destruction properly retains the old backing, all three assertions pass, then the new generation allocates the same XID and late old proof is routed by numeric ID to the new backing. This violates the reset invariant while passing the proposed test.

The test is feasible without prohibited production activation: `force_destroy_all_clients` accepts the real `Backend` trait object ([reset.rs lines 200–265](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver-core/src/core_loop/reset.rs:200)), and `KmsBackend::free_pixmap` already routes through the real store decrement path ([backend.rs lines 22440–22461](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/backend.rs:22440)). A fixture-installed managed allocation is not production Owner publication under R8.

Required correction: retain the three assertions, but cross the state-replacement boundary, deliberately reuse the numeric XID, and show that late proof destroys only the old incarnation’s entry while the new drawable/backing remains valid.

#### M-3 — Deferral leaves test-only Owner publication authorized by evidence that proves nothing

Section 6 acknowledges that `WriterCoverageProof` is empty and its test constructor requires no coverage, while `RecipientReservation` carries no identity, but defers both as “activation material” ([target lines 255–266](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-15-phase-c0-stage-2c-i-debt-design.md:255)).

Production issuers are correctly deferred by R8. The **test capabilities are not**: the authoritative Task 6 contract already requires tests to construct writer coverage only after explicit mock/disabled coverage for every class ([2c-i plan lines 1438–1446](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-09-phase-c0-stage-2c-i-resource-terminalization.md:1438)). The stage-2c design also requires all classes covered and the teardown receiver installed before Owner publication ([stage-2c design lines 274–288](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-08-phase-c0-stage-2c-conversions-and-damage-design.md:274)).

Concrete failure: Session 1 supplies the empty proof and identityless reservation, then proves every newly enumerated `issue_handover_permit` guard while never proving that all writer classes are disabled/mediated or that the recipient belongs to the device/incarnation. The test-only Owner route can therefore exercise and certify a state the authoritative handover contract forbids.

Required correction: keep production issuers deferred, but strengthen the test-only constructors now. Writer coverage must consume explicit evidence for every `WriterClass`; recipient reservation must identify a pre-existing compatible recipient slot and device/incarnation.

### Minor

None.

## Coverage and implementation checks

- **Incorporation:** audited all four prior findings; two applied and two partial.
- **Architecture/contracts:** checked handover authority, production-activation boundaries, reset/backend ownership, and proof delivery.
- **Safety/ownership:** checked husk token drop feasibility, identity correlation, premature backing destruction, stale-generation evidence, and transport closure semantics.
- **Specification/verification:** checked the census oracle, reset assertions and mutation, R7–R11, Task 6, stage-2c §6, and implementation gates.
- **Excerpts used:** **12/12** bounded excerpts, plus locator searches and the required single reads of target and prior review.
- **Verified ground:** fail-closed token drop has a working repository precedent; forced teardown reaches the real backend free path; retained backing and logical XID destruction are distinct; production Owner issuance remains forbidden.
- **Not assessed as sound:** all 67 census classifications, every mutation implementation, and detailed contents of the optional refusal inventory.
- **Deferred to implementation:** compilability, exact APIs/test placement, formatting, clippy, hardware and flake runs, full suites, and target portability.