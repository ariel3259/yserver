# Stage 3b design — codex review, round 5

**Target:** `docs/superpowers/specs/2026-09-24-phase-c0-stage-3b-modeset-and-randr-design.md`
revision 5 (`5fddc833`), prior review round 4.

**Result:** 1 blocking, 2 major, 0 minor; coverage INCOMPLETE (24/24; full copied
source/sink return path, gate interception of every RANDR mutation, C.0 §16/§18
gates unassessed). Trend: r1 2B 3M, r2 2B, r3 1B 1M, r4 2B 1M, r5 1B 2M.

**Reviewer:** `codex exec --sandbox read-only`, single pass
**Instrument:** `docs/superpowers/review/` @ `0245f96b`;
model `gpt-6-sol`; reasoning effort `xhigh`; `codex-cli 0.155.1`.
Counts are comparable only to other reviews citing this same instrument SHA.

**Author verification (2026-09-24):**

- **B-1 — CONFIRMED.** Revision 6 admits a position-only change on
  `Tier::Topology` with an empty description; it promotes only when the device
  slot is free, and advances the topology generation.
- **M-1 — CONFIRMED.** Revision 6 routes every late asynchronous completion by
  job identity to a kept output's `OutputKey` or to its retired bundle, which
  only services proofs.
- **M-2 — CONFIRMED.** `ResourceService::cancel` exists (`resources/mod.rs:920`);
  revision 6 cancels the rejected commit's `KmsRelease` registrations once.

## Verdict

**1 blocking, 2 major, 0 minor.** Coverage: **INCOMPLETE**.

This is a design-review result. It does not establish that the design compiles, passes tests, or is approved for implementation.

## Incorporation audit

| Prior finding | Status | Assessment |
| --- | --- | --- |
| Round 4 B-1, position-only scene replacement | **APPLIED** | Revision 5 updates the existing scene in place and preserves its resource owners. |
| Round 4 B-2, dark-to-dark pool release | **APPLIED** | A typed proof now requires a proven off state and a completed displacing commit; an unproven chain retains the pool. |
| Round 4 M-1, index-addressed retired output | **PARTIAL** | The retired bundle has an identity-stable route, but late copied-route completions still need an explicit route and disposition (M-1 below). |
| Round 3 B-1, gate expiry behind Legacy | **APPLIED** | Deadlines are serviced before the next admission after a synchronous Legacy call. |
| Round 3 M-1, disabled pool release | **PARTIAL** | Active and dark displacement have proofs; rejection does not account for the obligation registered at dispatch (M-2). |
| Round 2 B-1, scene resource handoff | **PARTIAL** | Scene retirement is specified, but a pending copied-route job can finish after its scene moves (M-1). |
| Round 2 B-2, validation at gate expiry | **APPLIED** | Expiry runs stateless checks only. |
| Round 1 B-1, atomic `EBUSY` retry | **APPLIED** | The design forbids retry and closes readiness. |
| Round 1 B-2, whole-request bound | **APPLIED** | The design names queue, probe, prerequisite, unflip, host-call, and completion deadlines, with the declared Legacy allowance. |
| Round 1 M-1, position-only request | **TRADED** | The KMS commit and scene replacement are gone, but promotion can bypass an occupied commit slot (B-1). |
| Round 1 M-2, fallible promotion | **APPLIED** | Scene construction precedes acceptance; promotion moves staged data. |
| Round 1 M-3, requester-less publication | **APPLIED** | Events reach the arbiter immediately; their publication passes through the gate. |

## Findings

### Blocking

**B-1 — Position-only promotion can overtake an accepted primary commit.** The design calls a position-only change class-1 work, but gives it no `Tier::Topology` dispatch and lets it promote by updating the live scene in place ([3b:191](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-24-phase-c0-stage-3b-modeset-and-randr-design.md:191), [3b:197](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-24-phase-c0-stage-3b-modeset-and-randr-design.md:197)). Its client-modeset slot is separate from the device commit slot ([3b:125](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-24-phase-c0-stage-3b-modeset-and-randr-design.md:125)). A composed flip can therefore be accepted, followed by position-only promotion and a RANDR reply, followed by completion of the frame prepared for the old origin. C.0 forbids topology work overtaking a `Submitting` or accepted commit ([C.0:1452](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:1452)). Make logical promotion wait for, or terminalize, the device’s preceding commit through the same class-1 barrier; it still needs no KMS property change. Test with an accepted composed flip held through the position request.

### Major

**M-1 — A late copied-route completion lacks a retired-output disposition.** Revision 5 moves the copied pool and scene into a detached bundle and requires bundle-addressed resource operations ([3b:413](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-24-phase-c0-stage-3b-modeset-and-randr-design.md:413)). It does not say how an already-running copy job is correlated after that move, or prohibit its completion from offering or submitting an old generation. The current completion path looks up `pending_acks` by output index and can proceed to `submit_copied_scanout` ([scene.rs:4531](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/scene.rs:4531), [scene.rs:4557](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/scene.rs:4557)). If disable or mode change promotes while a copy is waiting, its completion arrives after the index has shifted or points at the new pool. Define a job-identity route to the retired bundle that services source/sink proofs and terminalizes old work without a new KMS submission. Test a copy completion delayed past promotion. This also enforces C.0’s requirement to invalidate queued intents from older topology generations ([C.0:1452](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:1452)).

**M-2 — Explicit rejection leaves the old pool’s registered release obligation unspecified.** The design registers `KmsRelease` on each displaced allocation **at dispatch**, but the rejection rows say only that the prepared set is released ([3b:333](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-24-phase-c0-stage-3b-modeset-and-randr-design.md:333), [3b:480](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-24-phase-c0-stage-3b-modeset-and-randr-design.md:480)). After an explicit `EINVAL`, the old pool remains current, yet its obligation against the rejected commit can remain outstanding. A later successful replacement cannot discharge that earlier obligation, retaining the pool until a device barrier. The resource service has a distinct cancellation operation ([resources/mod.rs:920](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/resources/mod.rs:920)); C.0 treats explicit rejection as a never-installed state ([C.0:2161](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:2161)). Specify exact-once cancellation of the rejected commit’s old-pool registrations, and test rejection followed by a successful displacement.

### Minor

None.

## Coverage and implementation checks

**24/24 bounded spec/source excerpts used; no builds or tests run.** Incorporation covered the round 4 findings and their carried risks. Architecture checked the lifecycle boundary, device ordering, RANDR gate, and current completion routes. Safety checked dark release, scene retirement, copied work, and rejection. Compliance checked the relevant C.0 ordering and evidence rules and the umbrella’s six RANDR obligations.

Coverage is incomplete: the full copied-route source/sink return path, every RANDR mutation’s gate interception in the core loop, and the authoritative spec’s detailed §16/§18 gates were unassessed. None is judged sound by omission. Implementation must verify these contracts with real tests and run its assigned formatting, CI clippy, build, and portability gates.