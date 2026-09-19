# Stage 2c-iii plan Ci — codex review, round 2

**Target:** plan Ci revision 2 (`8f60bab8`), with round 1 as the prior review.

**Reviewer:** `codex exec --sandbox read-only`, single pass
**Instrument:** `docs/superpowers/review/` @ `da807b70`;
model `gpt-5.6-sol`; reasoning effort `xhigh`; `codex-cli 0.155.1`.
Counts are comparable only to other reviews citing this same instrument SHA.
**Coverage: INCOMPLETE** (24/24): whether the stub-executor owner fixture and the
live managed Vulkan platform can be combined was not inspected.

**Author verification (2026-09-19), every finding checked against the tree:**

- **B-1 — CONFIRMED.** A composed buffer enters Submitted inside the ledger
  closure, and a later pre-IPC `send_on` refusal returns the ledger with the
  generation still desired (2c-ii §6–§7); revision 2 had no Submitted → Desired
  exit and no buffer rule for the non-unknown invalidation sources. Fixed in
  decision 10; R33–R34.
- **M-1 — CONFIRMED.** `consume` records a `HardwareComplete` that arrives before
  retirement and discharges inside the `CompletionRetired` handler
  (`resources/commit.rs:227`–`262`); `Completed` requires `HardwareComplete`
  (`owner/record.rs`). R20 could not be killed at the scene. Fixed: reachable
  separations at the scene, gate 2 at the service (debt census); **the spec's
  §4.2 and §8.2 row are corrected** — its "each withheld alone" assumed
  independence the owner does not have.
- **m-1 — CONFIRMED.** Fixed: `C0-R8` for the normative rule.
- **Found by the author while verifying:** `for_tests_with_vk_live_scene`
  allocates unmanaged pools (no adoption in `backend.rs:6355`–`6425`), which the
  plan refuses for `Owner`, and the cited `scene.rs:8890` test builds its
  `PendingAck` by hand. Task 4 now builds an owner-route live fixture and bans
  hand-built state; F8 if adoption needs non-production entries.

Plan revision 3 incorporates all of it.

---

## Verdict

**1 blocking, 1 major, 1 minor**

Coverage: **INCOMPLETE**

This is a design-review result only; it does not claim compilation, test success, or implementation approval.

## Incorporation audit

| Prior finding | Status | Assessment |
|---|---|---|
| B-1 — owner BO lifecycle | **TRADED** | The never-admitted `Rendering/Desired → Displaced → Free` path is now explicit, but decision 10 omits the required post-`begin`, pre-IPC `Submitted → Desired` return and non-`CompletionUnknown` invalidation exits. |
| M-1 — per-member preservation | **APPLIED** | Task 2 now covers retirement, rejection/`ResourcesStillCurrent`, and direct subset dispatch, with R27–R28 ([plan lines 75, 122–129](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-19-phase-c0-stage-2c-iii-plan-ci-composed.md:75)). |
| M-2 — Owner eligibility | **APPLIED** | Copied, unmanaged, and mixed-output devices are covered by device-wide tests and R29–R30 ([plan lines 77, 161–169](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-19-phase-c0-stage-2c-iii-plan-ci-composed.md:77)). |
| M-3 — milestone evidence | **APPLIED** | R15 now mutates `Presented`; R31 and the F8 rule cover each reachable invalidation source ([plan lines 83–85, 202–211](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-19-phase-c0-stage-2c-iii-plan-ci-composed.md:83)). |
| M-4 — stable output identity | **APPLIED** | Members use `OutputKey`, CRTC, buffer and generation, with invalidation before reindex and R32 ([plan lines 47, 86](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-19-phase-c0-stage-2c-iii-plan-ci-composed.md:47)). |

## Findings

### Blocking

#### B-1 — Decision 10 still is not a total owner-buffer lifecycle

A composed generation enters `Submitted` inside the ledger closure, before `send_on` ([plan lines 38–40, 187, 200](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-19-phase-c0-stage-2c-iii-plan-ci-composed.md:38)). A subsequent pre-IPC `send_on` refusal terminalizes the record as `NeverDispatched`, returns its ledger, aborts admission, and—normatively—leaves composed desired state desired ([2c-ii spec lines 269–297, 317–325](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-18-phase-c0-stage-2c-ii-admission-design.md:269)). Decision 10 instead puts “pre-IPC refusal keeps it Desired” on the `Desired` row, which is no longer the buffer’s state, and gives `Submitted` only kernel rejection → `Displaced`.

Concrete sequence: `Desired → Submitted`; `send_on` returns `TransportGateRefused`; owner returns the new resources. The implementation must now either withdraw the generation as `Displaced`, violating 2c-ii latest-wins/refusal semantics, or restore `Desired` through an unspecified transition, risking duplicate ownership or loss of the returned allocation. None of the named tests exercises a post-`begin` pre-IPC refusal with a real composed member.

The same table only maps `CompletionUnknown` to `Quarantined`. Topology, VT release, device loss, incarnation poison, and recovery invalidate the transaction under the spec ([spec lines 326–334](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-19-phase-c0-stage-2c-iii-conversion-design.md:326)), but Task 6 only defines damage invalidation and Task 7 only promises “unknown exits” ([plan lines 208–209, 221–231](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-19-phase-c0-stage-2c-iii-plan-ci-composed.md:208)). An owner BO can therefore remain permanently `Submitted/Accepted/Current`, or be passed through the legacy any-state reset to `Free` without owner release proof ([scanout.rs lines 441–448](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/vk/scanout.rs:441)).

Required correction: add explicit, exclusive transitions for:

- `begin` refusal: remain `Desired`;
- post-`begin` pre-IPC `NeverDispatched`: `Submitted → Desired`, restoring the returned resources exactly once;
- post-IPC/kernel rejection: `Submitted → Displaced`;
- every invalidation source: a defined safe release or entry into `Quarantined`, with recovery out of it still deferred to stage 3.

Add a routed pre-IPC refusal test and make invalidation tests assert BO/allocation state, not only damage and `owes_repaint`.

### Major

#### M-1 — The three-gate test cannot independently prove the `CompletionRetired` gate

The plan requires three subcases with exactly one gate withheld and mutations R19–R21 ([plan lines 88, 222–231](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-19-phase-c0-stage-2c-iii-plan-ci-composed.md:88)); the spec likewise requires independently withheld gates ([spec lines 336–346, 567–568](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-19-phase-c0-stage-2c-iii-conversion-design.md:336)).

Today, if `HardwareComplete` arrives before `CompletionRetired`, the consumer merely remembers it because the old resources and their obligations are still owner-held ([commit.rs lines 227–242](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/resources/commit.rs:227)). When `CompletionRetired` later returns the ledger, that same handler discharges the KMS obligation and moves the old resources to releasing ([commit.rs lines 245–295](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/resources/commit.rs:245)). Thus production cannot reach “KMS discharged, GPU retired, but no `CompletionRetired`.” An R19 implementation that omits only the explicit retirement gate still remains blocked by the outstanding KMS obligation, so the claimed mutation can survive.

Required correction: define an authentic production-path mechanism that can record/discharge commit-keyed KMS evidence independently of ledger return, or redesign R19’s evidence so it genuinely isolates release-before-`CompletionRetired` without a test-only bypass.

### Minor

#### m-1 — “R8” names two unrelated obligations

“Production activation (R8)” appears in the limits, while mutation R8 means allowing Owner for a copied output; the coordinator is then told to apply R1–R32 by line ([plan lines 52, 77, 280](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-19-phase-c0-stage-2c-iii-plan-ci-composed.md:52)). This makes audit provenance ambiguous. Rename the mutation series `Ci-R1…Ci-R32` or qualify the normative requirement as `C0-R8`.

## Coverage and implementation checks

- Incorporation: all five prior findings checked.
- Architecture: checked producer/drain routing, ledger return semantics, BO phases, resource consumption, and gate ordering.
- Safety: checked refusal, rejection, invalidation, quarantine, fence ownership, and reuse gates.
- Verification: checked named tests and R1–R32 against §§4.0–4.6 and §8.2.
- Excerpts used: **24/24**.
- Verified ground: the production render-completion entry drains into the scene ([backend.rs lines 20648–20674](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/backend.rs:20648)); the cited Vulkan fixture manually constructs `PendingAck`/`BoPhase` and is not itself real-tick evidence ([scene.rs lines 8906–8947](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/scene.rs:8906)).
- Unassessed: the excerpt cap prevented inspecting whether `admission_backend_with_stub_executor` can be combined with the live managed Vulkan platform and drive tick → production drain → composed offer without manual state injection. That remains the specific authorized follow-up question, not verified ground.
- The upstream C.0 §§10.2/10.4/12.1 and stage-2c document were not independently reread after the cap; they are not deemed sound.
- Exact Rust signatures, compilation, clippy, portability, and runtime behavior remain deferred to implementation.