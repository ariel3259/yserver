# Plan Cfb — codex review, round 1

**Target:** plan Cfb revision 1 (`e3746d2f`), against the direct-framebuffer
adoption design revision 4 (`ec97fc88`).

**Reviewer:** `codex exec --sandbox read-only`, single pass
**Instrument:** `docs/superpowers/review/` @ `da807b70`;
model `gpt-5.6-sol`; reasoning effort `xhigh`; `codex-cli 0.155.1`.
Counts are comparable only to other reviews citing this same instrument SHA.
Coverage INCOMPLETE (24/24): whether the Owner live fixture can produce
composed -> direct A -> direct A under the frontmost-window rule, and the full
storage-replacement enumeration, were not reached.

**Author verification (2026-09-21), every finding checked against the tree:**

- **B-1 — CONFIRMED.** `into_managed` has no producer before Task 3, so Task 1
  could not obtain a payload without building one. Fixed: tasks reordered
  (adoption producer in Task 1; pending cleanup in Task 2; serial/index/step in
  Task 3), each with its own tests.
- **B-2 — CONFIRMED.** `AllocationLease::drop` only inserts the key into the
  weak dirty queue (`lease.rs:47`); both completion sites discard
  `service_completions`' keys (`backend.rs:21704`, `:21715`). Fixed: decision 11,
  one authoritative service step; F31/F32.
- **M-1 — CONFIRMED.** `reserve` discriminates only by `UseKind`. Fixed:
  decision 10, a role-proof lease entry; generic `reserve` refuses framebuffer
  payloads; F9 is that refusal.
- **M-2 — CONFIRMED.** Fixed: F4 = skip a bump; F12a/F12b for the key.
- **M-3 — CONFIRMED.** Fixed: checked increment, exhaustion refuses adoption.

Revision 2 incorporates all five.

---

## Verdict

**2 blocking, 3 major, 0 minor**

**Coverage: INCOMPLETE**

This is a design-review result only; it does not claim compilation, passing tests, or implementation approval.

## Incorporation audit

| Prior finding | Status | Assessment |
|---|---|---|
| None | N/A | First review; check 1 was skipped as instructed. |

## Findings

### Blocking

#### B-1 — The task order cannot satisfy its own production-producer-only test rule

Task 1 must test failed cleanup and freeze handoff using a `DirectFramebufferAllocation` ([plan line 96](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-21-phase-c0-stage-2c-iii-plan-cfb-direct-framebuffer-adoption.md:96)), while the global constraint prohibits hand-built allocations and requires every object to come through `managed_prepare_direct_candidate` and the direct offer path ([plan line 38](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-21-phase-c0-stage-2c-iii-plan-cfb-direct-framebuffer-adoption.md:38)). The baseline has no production caller of `into_managed` ([spec line 31](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-21-phase-c0-stage-2c-iii-direct-framebuffer-adoption-design.md:31)); that producer is only added in Task 3 ([plan line 128](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-21-phase-c0-stage-2c-iii-plan-cfb-direct-framebuffer-adoption.md:128)).

Concrete failure: Task 1 reaches its red-test step, but cannot produce the payload whose failed cleanup it claims to test. It must either violate the fixture rule or stop with F8. Task 2 has the same ordering smell: its core token/count tests are explicitly postponed to Task 3, leaving its index implementation without its own gate ([plan line 122](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-21-phase-c0-stage-2c-iii-plan-cfb-direct-framebuffer-adoption.md:122)).

Smallest correction: restructure the task graph so the first task promising pending-cleanup/index tests also installs the production adoption producer—most simply merge the registry, index and adoption slice—or explicitly limit earlier tasks to scaffolded interfaces and defer both implementation and acceptance of producer-dependent behavior.

#### B-2 — No event-loop contract delivers last-lease retirement to index removal and registry cleanup

The plan requires service-observed `1 → 0` to remove the M1 index ([plan line 120](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-21-phase-c0-stage-2c-iii-plan-cfb-direct-framebuffer-adoption.md:120); [spec line 128](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-21-phase-c0-stage-2c-iii-direct-framebuffer-adoption-design.md:128)). Today, dropping a lease only removes the use and inserts its key into a weak dirty queue ([lease.rs line 47](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/resources/lease.rs:47)). `service_ready`/`service_ready_with_registry` consume that queue ([mod.rs line 824](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/resources/mod.rs:824)), but both backend completion-driving sites discard `service_completions`’ returned keys ([backend.rs line 21703](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/backend.rs:21703)). The plan assigns no typed event, callback, polling order, or registry-aware tick connecting these components.

Concrete failure: a never-dispatched successor drops its sole lease; the key becomes dirty; the ordinary service pass encounters a file-owned payload and re-dirties it, while the backend discards the transition. The index is not removed and registry cleanup is not driven. A subsequent lookup can either observe a stale entry or remint from a zero-use entry, depending on the unspecified token semantics.

Smallest correction: define one authoritative backend service step, used at both completion sites, that drives readiness with the paired registry, emits an unambiguous last-direct-lease transition, removes every matching M1 token, and only then completes registry cleanup. Specify its ordering relative to managed lookup and incarnation teardown.

### Major

#### M-1 — “Only direct holders lease” has neither an enforceable boundary nor a killable F9

The service’s generic `reserve(key, UseKind)` accepts callers without payload or holder-role discrimination ([mod.rs line 485](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/resources/mod.rs:485)); compatibility tracks only `UseKind` ([availability.rs line 115](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/resources/availability.rs:115)). The plan leaves the choice between refusal and “making the count observably wrong” to implementation and names no reachable non-direct consumer or observation sequence ([plan line 120](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-21-phase-c0-stage-2c-iii-plan-cfb-direct-framebuffer-adoption.md:120)).

An accidental non-direct `Retain` lease makes a direct drop appear as `2 → 1`, permanently retaining the index. A normal-path test can still pass because it never introduces such a caller; F9 is an unspecified code addition, not a line mutation with a defined observable.

Require either a framebuffer-specific leasing capability unavailable to generic consumers, or name the exact reachable mutation site and assert that the injected lease prevents the expected last-drop removal.

#### M-2 — F4 cannot be killed by its assigned backing-serial test

F4 removes the serial from the cache key, but its named test only verifies that the serial changes ([plan line 75](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-21-phase-c0-stage-2c-iii-plan-cfb-direct-framebuffer-adoption.md:75)). The serial can continue incrementing while lookup keys only by `DrawableId`; that test passes. The required behavioral property is that replacement forces a different import/key ([spec line 303](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-21-phase-c0-stage-2c-iii-direct-framebuffer-adoption-design.md:303)).

Split the evidence: mutate away a serial bump for the store test, and assign “omit serial from index key” to the relayout/re-import fresh-key test.

#### M-3 — The `u64` backing serial lacks the required non-repetition/exhaustion rule

The plan specifies a bumped `u64` ([plan line 24](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-21-phase-c0-stage-2c-iii-plan-cfb-direct-framebuffer-adoption.md:24)), while the spec requires that it never repeat ([spec line 120](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-21-phase-c0-stage-2c-iii-direct-framebuffer-adoption-design.md:120)). No checked-overflow or fail-closed disposition is defined. Release wrapping could reuse an old cache identity.

Specify checked increment; exhaustion must permanently disable direct adoption for that drawable/device path or otherwise allocate a provably fresh identity.

### Minor

None.

## Coverage and implementation checks

- Architecture and safety covered task dependencies, service/index event delivery, pending-cleanup ownership, alias/barrier shape, lease boundaries, raw-storage mutation evidence, and task gates.
- Spec coverage included §§1.1, 2.1–2.6, 3.1–3.4 and 4.2–4.3.
- F14 is viable: `root_storage_extent` dereferences raw storage ([backend.rs line 14007](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/backend.rs:14007)), and managed storage panics through `Deref` ([store.rs line 132](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/store.rs:132)).
- Excerpts: **24/24** slots used. One resource-service retrieval inadvertently spanned 156 lines rather than 120; investigation stopped at the budget.
- Unassessed: whether the Owner live fixture can produce composed → direct A → direct A through the frontmost-window rule, and the complete storage-replacement enumeration. The specific unresolved question is whether `c0_conv_cii_try_candidate`/`admission_direct_candidate` can generate both same-source commits without a synthetic entry.
- Compilation details, portability, mutations, GPU behavior and test execution remain deferred to implementation.