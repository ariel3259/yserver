# Plan Cfb — codex review, round 2

**Target:** plan Cfb revision 2 (`39f32083`), with round 1 as the prior review.

**Reviewer:** `codex exec --sandbox read-only`, single pass
**Instrument:** `docs/superpowers/review/` @ `da807b70`;
model `gpt-5.6-sol`; reasoning effort `xhigh`; `codex-cli 0.155.1`.
Counts are comparable only to other reviews citing this same instrument SHA.
Coverage INCOMPLETE (24/24): the full frontmost-window eligibility path and the
storage-replacement enumeration were not reached.

**Author verification (2026-09-21), every finding checked against the tree:**

- **B-1 — CONFIRMED.** The scene's completion handler holds only
  `&mut ResourceService` (`scene.rs:3856`); readiness destroys before returning
  (`resources/mod.rs:868`); the backend sites service before
  `route_owner_event_batch`. Fixed: decision 11 is a two-phase protocol (zero
  edges recorded in the service; the backend step after the batch); F31 per site,
  F32, F33.
- **M-1 — CONFIRMED.** `RoleReservation` carries role, serial, closed cell and
  discharge state only. Fixed: `DirectLeasePermit` bound to capacity, device/
  incarnation, role state and service; F9a–F9c.
- **M-2 — CONFIRMED.** An exhausted service is refused at preflight. Fixed: the
  generation counter seeded at its last value.
- **M-3 — CONFIRMED, root cause in hardware.** `probe_direct_scanout_test_only`
  imports (master-free) and then runs `TEST_ONLY` (master needed) in one
  function; the Cii helpers fake the entry for that reason. Fixed: decision 13
  splits the probe; the fixture drives the real import and omits only the
  validation; spec 4.2a records it. F34, F35.

Revision 3 incorporates all four.

---

## Verdict

**1 blocking, 3 major, 0 minor**

**Coverage: INCOMPLETE**

This is a design-review result only; it does not claim compilation, passing tests, or implementation approval.

## Incorporation audit

| Prior finding | Status | Assessment |
|---|---|---|
| B-1 — task order versus production-producer-only tests | APPLIED | Adoption moved to Task 1; pending cleanup and index work now consume that earlier producer ([plan 104–145](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-21-phase-c0-stage-2c-iii-plan-cfb-direct-framebuffer-adoption.md:104)). |
| B-2 — no event-loop delivery of last-lease retirement | PARTIAL | Decision 11 states the desired ordering, but does not provide a workable boundary across the scene-owned completion calls and backend-owned index/registry. See B-1. |
| M-1 — unenforced direct-holder-only leasing | PARTIAL | Generic `reserve` is refused, but `RoleReservation` is not a service-verifiable, device-bound proof. See M-1. |
| M-2 — F4 did not test index identity | APPLIED | Store bump evidence and index-key mutations are separated into F4 and F12a/F12b ([plan 82, 88](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-21-phase-c0-stage-2c-iii-plan-cfb-direct-framebuffer-adoption.md:82)). |
| M-3 — serial repetition on overflow | APPLIED | Checked increment, permanent drawable refusal, and an exhaustion test are specified ([plan 29, 82](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-21-phase-c0-stage-2c-iii-plan-cfb-direct-framebuffer-adoption.md:29)). |

## Findings

### Blocking

#### B-1 — The authoritative service step has no cross-layer execution contract

Decision 11 requires four callers to remove M1 tokens before registry cleanup ([plan 32, 89](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-21-phase-c0-stage-2c-iii-plan-cfb-direct-framebuffer-adoption.md:32)), matching the spec’s `1 → 0` and cleanup ordering ([spec 128–147, 278–281](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-21-phase-c0-stage-2c-iii-direct-framebuffer-adoption-design.md:128)). But the scene completion function receives only `&mut ResourceService`, not the backend M1 index or registry ([scene.rs 3856–3861](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/scene.rs:3856)), and must service completion before publishing the offer ([scene.rs 3959–3972](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/scene.rs:3959)). Meanwhile, service readiness currently performs destruction before returning keys ([resources/mod.rs 868–911](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/resources/mod.rs:868)).

There is also an ordering hole at the backend sites: servicing occurs before `route_owner_event_batch` ([backend.rs 21703–21720](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/backend.rs:21703)). An owner event can therefore drop the final lease after the pass; a later Present can upgrade the still-indexed token before the next pass. Because the dirty set records only the key, the later service sees a live use and cannot reconstruct the intervening `1 → 0`.

Smallest correction: define an explicit two-phase service protocol. Lease drop must record a typed zero edge; readiness must return zero edges and releasable payloads without destroying them; the backend removes all matching indices, then authorizes registry cleanup. Specify how scene completion synchronously crosses this boundary without reversing its completion-before-offer ordering, and run F31 independently through all four call sites.

### Major

#### M-1 — `RoleReservation` is not a verifiable role proof

The dedicated lease entry accepts the holder’s `RoleReservation` as proof ([plan 31, 106–112](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-21-phase-c0-stage-2c-iii-plan-cfb-direct-framebuffer-adoption.md:31)). That object carries only role, serial, an admission-closed cell, and discharge state ([capacity.rs 36–61](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/resources/capacity.rs:36)); it contains no capacity identity, device, incarnation, or service binding. The service therefore cannot distinguish the correct candidate’s live reservation from one issued by another capacity/device.

A foreign reservation can mint an unrelated framebuffer lease; after the real holder drops, the service observes `2 → 1`, retaining the index contrary to spec §2.3. F9 tests only generic `reserve`, so this misuse survives.

Smallest correction: use an opaque permit bound to the issuing capacity, device/incarnation, current role state, and service. Add rejection tests for foreign, stale, and wrong-role permits.

#### M-2 — Task 1’s post-conversion adoption-failure test has no defined trigger

Task 1 checks “service not exhausted” before `into_managed`, yet proposes an exhausted service to force failure after conversion ([plan 110, 116](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-21-phase-c0-stage-2c-iii-plan-cfb-direct-framebuffer-adoption.md:110)). Existing adoption rejects an already-exhausted service before allocating a key ([resources/mod.rs 368–393](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/resources/mod.rs:368)).

Consequently the test exercises step-1 refusal, never the sole-payload cleanup/admission-close exit; F17 can survive.

Smallest correction: name a deterministic state that passes preflight but fails adoption—for example checked generation/use-ID exhaustion—or specify a production-path fault injection that does not bypass `into_managed`.

#### M-3 — No compliant producer is assigned for the Vulkan and Ciii re-entry tests

The plan forbids hand-built M1 entries and requires production entries ([plan 44–49](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-21-phase-c0-stage-2c-iii-plan-cfb-direct-framebuffer-adoption.md:44)). Both available candidate helpers violate that rule: `c0_conv_cii_try_candidate` constructs fake FB/GEM handles and inserts an accepted cache entry ([backend.rs 52387–52406](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/backend.rs:52387)); `admission_direct_candidate` delegates to `managed_prepare_ready_candidate`, which explicitly creates and inserts a fake accepted framebuffer ([backend.rs 50212–50215, 50274–50294](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/backend.rs:50212)).

Thus Tasks 1, 3, 4, and especially Task 5 lack an identified compliant setup for same-source reuse. The named re-entry test may stop at F8 instead of testing F11/P3-3.

Smallest correction: Task 1 must establish a reusable fixture helper that drives the real eligibility/probe/admission path with a frontmost window and actual imported framebuffer; later tasks must name that helper.

### Minor

None.

## Coverage and implementation checks

- All four requested checks were performed: five prior findings audited; task contracts and event-loop ownership traced; lease/cleanup ordering examined; spec §§2.1–2.6, 3.1–3.4, and 4.2–4.3 checked.
- Excerpts used: **24/24**.
- Verified ground: role-reservation shape, lease dirtying, readiness/destruction order, all four completion calls, scene offer ordering, Owner fixture installation, and existing candidate helpers.
- Unassessed—not sound: the full frontmost-window eligibility path and complete storage-replacement enumeration. The specific unresolved question is whether another existing Owner-live production probe path can produce composed → direct A → direct A without synthetic M1 insertion.
- Exact Rust signatures/borrows, compilation, clippy, portability, GPU behavior, mutation execution, and test results remain deferred to implementation.