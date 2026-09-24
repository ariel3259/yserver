# Stage 3b design — codex review, round 7

**Target:** `docs/superpowers/specs/2026-09-24-phase-c0-stage-3b-modeset-and-randr-design.md`
revision 7 (`644ae398`), prior review round 6.

**Result:** 0 blocking, 1 major, 0 minor; coverage INCOMPLETE (24/24; the gate
interception of every configuration-changing RANDR request — delegated to the
plan — and the copied destination ledger after an unsubmitted B unassessed).
Trend: r1 2B 3M, r2 2B, r3 1B 1M, r4 2B 1M, r5 1B 2M, r6 2B 1M, r7 0B 1M.

**Reviewer:** `codex exec --sandbox read-only`, single pass
**Instrument:** `docs/superpowers/review/` @ `0245f96b`;
model `gpt-6-sol`; reasoning effort `xhigh`; `codex-cli 0.155.1`.
Counts are comparable only to other reviews citing this same instrument SHA.

**Author verification (2026-09-24):**

- **M-1 — CONFIRMED.** Revision 8 gives the retired copied pool a
  stage-by-stage disposition that reuses 2c-iii's lifecycle-quiescence
  normalization (`vk/scanout.rs:1339`, tested at `:7881`/`:7899`) on per-fence
  proof, and covers the unsubmitted-B case (source released on B's read
  obligation, `copied_owner.rs:44`).
- **Review loop closed at round 7** (first round with no blocking finding),
  as 3a closed at its first 0-blocking round. The two unassessed areas become
  plan obligations: the plan enumerates every configuration-changing RANDR
  request and proves each enters the gate before state-dependent validation,
  and its tests cover each retired copied stage.

## Verdict

**0 blocking, 1 major, 0 minor.** Coverage: **INCOMPLETE**.

This is a design review. It does not establish that revision 7 compiles, passes tests, or is approved for implementation.

## Incorporation audit

| Prior finding | Status | Assessment |
| --- | --- | --- |
| Round 5 B-1, position-only ordering | APPLIED | Position-only work enters `Tier::Topology`, waits for the device slot, and sends no KMS request (plan lines 193–216, 727–730, 758–760). |
| Round 5 M-1, late copied completion | APPLIED | Job identity routes a late copy completion to its retired bundle, where it cannot offer or submit (lines 451–462, 761–764). The earlier finding is addressed; the distinct earlier *source-render* stage is M-1 below. |
| Round 5 M-2, rejected displacement | APPLIED | Rejection cancels the commit’s old-pool `KmsRelease` registrations (lines 521, 754–757). |
| Round 4 B-1, position-only scene replacement | APPLIED | The scene state is updated in place (lines 210–215, 770–773). |
| Round 4 B-2, dark-to-dark pool release | APPLIED | The typed proof requires a proven-off chain and a completed displacing commit (lines 366–400). |
| Round 4 M-1, index-addressed retirement | APPLIED | Retired resources use a bundle and retirement identity (lines 435–449, 774–777). |
| Round 3 B-1, expiry behind Legacy | APPLIED | Expired deadlines are serviced before the next admission after a synchronous Legacy call (lines 657–669). |
| Round 3 M-1, disabled pool release | APPLIED | Lit, dark, and rejected displacements have stated release rules (lines 355–400, 521). |
| Round 2 B-1, scene resource handoff | APPLIED | Replaced scene state moves to a retirement list with its pool (lines 417–449). |
| Round 2 B-2, validation at gate expiry | APPLIED | Expiry performs stateless checks only (lines 637–647). |
| Round 1 B-1, atomic `EBUSY` retry | APPLIED | No retry; readiness closes (line 520). |
| Round 1 B-2, whole-request bound | APPLIED | Queue, probe, wait, unflip, executor, and completion stages have deadlines or an immediate refusal (lines 618–669). |
| Round 1 M-1, position-only request | APPLIED | Its logical transaction and evidence now agree (lines 193–218, 727–730). |
| Round 1 M-2, fallible promotion | APPLIED | Scene construction and projection staging precede acceptance; promotion is specified as infallible (lines 257–274, 303–315, 329–353). |
| Round 1 M-3, requester-less publication | APPLIED | Events reach the arbiter immediately; publication is ordered through the gate (lines 685–705). |

Round 6’s new B-1, B-2, and M-1 corrections are also present in the failure table, staged-projection contract, and position-only test respectively (plan lines 257–274, 517–524, 727–730).

## Findings

### Blocking

None demonstrated.

### Major

**M-1 — Retired copied work lacks a disposition when source rendering finishes before the sink copy starts.** The design routes late work to a retired-output bundle and says that work only services proofs, with no new offer or KMS submission ([plan lines 435–462](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-24-phase-c0-stage-3b-modeset-and-randr-design.md:435)). Consider a copied output disabled after source render A is submitted but before A’s completion is drained. A completes after retirement. The copied source’s successful-render state awaits a sink handoff; the normal source and destination receipt is created only when sink copy B is prepared and submitted ([scanout.rs lines 664–677](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/resources/scanout.rs:664), [copied_owner.rs lines 312–341](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/copied_owner.rs:312), [lines 434–463](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/copied_owner.rs:434)). The bundle contract does not say who cancels that never-started B handoff or what proof returns A’s foreign ownership. Simply dropping the pool would conflict with C.0’s separate GPU and external-ownership return rules ([C.0 lines 2113–2134](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:2113), [lines 2153–2166](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md:2153)); retaining it without a terminal path can strand the old pool. The proposed late-copy test covers B completion, not this earlier completion ([plan lines 761–764](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-24-phase-c0-stage-3b-modeset-and-randr-design.md:761)).

Specify the retired bundle’s stage-specific disposition for **A completed, B never submitted**, including the proof and owner of the source return, and test a disable and mode change at that boundary. State separately how a completed B copy is retired when its destination was never submitted to KMS.

### Minor

None.

## Coverage and implementation checks

**24/24 spec/source excerpt slots used.** Incorporation covered every finding carried by round 6. Architecture checked the device-local modeset, result boundary, projection staging, and gate design against the umbrella’s six RANDR obligations ([umbrella lines 245–309](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-22-phase-c0-stage-3-lifecycle-design.md:245)). Safety checked copied source and sink proof paths, pool retirement, and the relevant C.0 release milestones. Compliance checked ordering, completion evidence, DPMS inheritance, deadlines, and Legacy client parity. One requested copied-owner excerpt exceeded the 120-line cap and was truncated; shorter follow-up excerpts supplied its relevant portions.

Coverage remains **incomplete**: the budget did not establish that every configuration-changing RANDR request enters the gate before state-dependent validation, or fully trace the copied destination ledger after B completes without a KMS submission. Those paths are unverified, not judged sound. The focused follow-up questions are those two contracts. Compiler, regular CI clippy, formatting, test, portability, and authorized hardware gates belong to implementation; none were run here.