# Copied scanout route design — codex review, round 2

**Target:** `docs/superpowers/specs/2026-09-22-phase-c0-stage-2c-iii-copied-route-design.md`
revision 2 (`d61fa1e4`), against the 2c-iii conversion design as the passed parent.
**Prior:** round 1 (`../findings/2026-09-22-stage-2c-iii-copied-route-design-review-round1.md`).

**Reviewer:** `codex exec --sandbox read-only`, single pass
**Instrument:** `docs/superpowers/review/` @ `da807b70`;
model `gpt-5.6-sol`; reasoning effort `xhigh`; `codex-cli 0.155.1`.
Counts are comparable only to other reviews citing this same instrument SHA.
Coverage COMPLETE FOR DECLARED SCOPE, 24/24 excerpts used; `CompletionPoller`
internals, device teardown, transport encoding, unrelated owner-ledger paths and
the delegated `FenceTicket`/fixture choices are recorded as unassessed.

**Author verification (2026-09-22), checked against the tree:**

- **B-1 — CONFIRMED, and revision 2's premise was worse than the finding
  says.** The review is right that CP-4 and CP-4a did not name one valid owner
  for the source lease across A's retirement: a lease placed in A's batch is
  consumed when servicing drops it (`resources/mod.rs:1560`), and one withheld
  from it makes A's batch not the retirement authority CP-4 claimed. Verifying
  it showed that CP-4a's own premise — that the producer holds live leases at
  the A-to-B boundary that can be moved — **is false**. The acquisition token's
  leases protect selection only and are dropped there, by design and with the
  reason written in the code: "retaining this lease across that reservation
  would make the service correctly report Busy for the same allocation"
  (`scene.rs:6194`-`6201`, `drop(token.display)`). The composed producer then
  reserves its own write lease and registers its own obligation through
  `prepare_retirement_batch` (`scene.rs:9807`, `resources/gpu.rs:299`).
  There was never a lease to move. Fixed: section 3.2 rewritten a second time,
  from the acquisition/compose/completion sequence the code actually runs.
- **B-1's second half — CONFIRMED.** `ResourceService::register`
  (`resources/mod.rs:865`) fails on wrong incarnation, exhaustion, detachment
  and freezing, so minting obligations after submission can leave GPU work with
  one obligation or none. The project already has the answer and revision 2 did
  not use it: `prepare_retirement_batch` registers every obligation **before**
  the caller may hand a raw handle to the GPU and unwinds a partial attempt with
  `cancel_prepared_entries`, so "nothing partially reserved survives a failed
  prepare" (`resources/gpu.rs:293`-`299`). Fixed: the copied route follows that
  idiom rather than inventing a transaction.
- **Incorporation audit M-1 and M-2 — agreed APPLIED.** The review also
  verified something revision 2 only assumed: the service's pending-batch
  deadline is part of the backend's wake calculation (`resources/mod.rs:352`,
  `backend.rs:22422`), so a batch whose wake registration failed is still
  serviced.

**What this round cost in confidence, recorded deliberately:** revision 2's
correction to a blocking finding rested on a premise its author did not check in
the code, and the review caught the consequence rather than the premise. The
lesson is the project's own and is now applied here: when a fix introduces a
mechanism, verify the mechanism's preconditions in the tree before writing it
down, not after the next round names them.

Revision 3 incorporates this finding.

---

## Verdict

**1 blocking, 0 major, 0 minor**

Coverage: COMPLETE FOR DECLARED SCOPE

This reviews the design only; it does not claim compilation, test success, or implementation approval.

## Incorporation audit

| Prior finding | Status | Assessment |
|---|---|---|
| B-1 — A→B handoff lacked an ownership protocol | **PARTIAL** | CP-4a now forbids re-acquisition and release-before-transfer, and requires a synchronous by-value handoff. However, it does not reconcile A’s batch ownership with B’s ownership of the same source lease, and places fallible obligation registration after B submission without a closed failure transaction. Carried forward as B-1 below. |
| M-1 — Post-submit wake-registration failure lacked an owner | **APPLIED** | Section 3.3 now displaces the generation, forbids an offer, removes partial waiter state, and leaves the submitted batch registered. The existing service supplies a periodic pending-batch deadline, and that deadline is included in the backend wake calculation ([resources/mod.rs:352](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/resources/mod.rs:352), [backend.rs:22422](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/backend.rs:22422)). |
| M-2 — Evidence could not prove the destination obligation | **APPLIED** | Section 6 now requires registration under the destination’s own key followed by retirement, and §8.2 separately mutates omission or miskeying of that obligation ([design:272](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-22-phase-c0-stage-2c-iii-copied-route-design.md:272), [design:324](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-22-phase-c0-stage-2c-iii-copied-route-design.md:324)). |

## Findings

### Blocking

#### B-1 — CP-4a is not a closed A→B ownership transaction

The revised design simultaneously says that A’s batch retires A’s leases, that B owns both allocations during the copy, and that the producer’s two existing leases are moved by value into B ([design:141](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-22-phase-c0-stage-2c-iii-copied-route-design.md:141), [design:152](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-22-phase-c0-stage-2c-iii-copied-route-design.md:152)). Those statements do not identify a valid owner for the source lease across A’s retirement.

The acquisition token owns the destination and source leases by value ([scanout.rs:578](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/resources/scanout.rs:578)). A `CoreRetirementBatch` likewise owns its leases, while `ReadObligation` itself owns the source lease ([gpu.rs:89](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/resources/gpu.rs:89), [gpu.rs:133](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/resources/gpu.rs:133)); successful servicing consumes and drops the batch ([resources/mod.rs:1540](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/resources/mod.rs:1540)).

Concrete failure sequence:

1. If the source lease is put in A’s batch to cover A’s write, servicing A consumes that value; it cannot then be moved into B’s `ReadObligation`.
2. If it is withheld from A’s batch for later transfer, A’s batch is not the lease-retirement authority CP-4 claims, and the design does not define the split ownership contract between that batch and the generation.
3. Re-acquisition cannot repair either ordering because CP-4a correctly establishes that it returns `Busy`.

The same transaction has a second uncovered boundary. CP-4a requires destination and source obligations to be minted only **after** B has submitted ([design:167](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-22-phase-c0-stage-2c-iii-copied-route-design.md:167)). `ResourceService::register` is fallible for wrong incarnation, exhaustion, detachment, or freezing ([resources/mod.rs:865](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/resources/mod.rs:865)). Existing GPU preparation instead registers obligations before exposing allocations to GPU work and atomically unwinds partial preparation ([gpu.rs:300](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/resources/gpu.rs:300)).

Thus B may submit successfully and then register only one—or neither—obligation. “Register the batch with whatever it holds” does not state whether the generation is displaced, which entries are frozen or cancelled, or what prevents destination retirement from authorizing an offer when the source obligation failed. No-yield execution does not make fallible operations atomic.

This defeats the authoritative readiness requirement that an offer follow all completed producer waits ([authoritative spec:307](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-19-phase-c0-stage-2c-iii-conversion-design.md:307)) while producer completion remains outside the owner ioctl ([authoritative spec:389](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-19-phase-c0-stage-2c-iii-conversion-design.md:389)).

Smallest correction: define one exact owner of each lease during A and an explicit service transition that yields or escrows those leases for B. Pre-register both B obligations while the existing leases exclude reuse, then specify cancellation after a proven-unsent/quiesced failure and quarantine after uncertain dispatch. If post-submit registration is retained, every partial registration outcome needs a fail-closed disposition and evidence mutation. Section 8.2 must cover these registration failures, not only re-acquisition and release-before-transfer.

### Major

None.

### Minor

None.

## Coverage and implementation checks

- **Incorporation:** audited all three round-1 findings; two are applied and B-1 remains partial.
- **Architecture/contracts:** checked producer stages, token/batch ownership, copied-pool adoption into one resource service, offer correlation, and deadline integration.
- **Safety/failure:** checked lease movement, obligation registration failures, batch validation/drop/quarantine, wake-registration failure, cancellation, and serviced-time recovery.
- **Compliance/evidence:** checked the parent’s producer/readiness, exclusivity, hardware, and mutation requirements against CP-1–CP-11.

Excerpts used: **24/24** beyond the target and prior review. Verified ground includes same-service adoption of both copied-pool halves, the real batch ownership model, fallible registration, and periodic servicing after wake-registration failure.

The budget was exhausted. CompletionPoller internals, full device-teardown behavior, transport encoding, unrelated owner-ledger paths, and the delegated FenceTicket/fixture construction choices remain unassessed and are not deemed sound.

Builds, Rust signatures and borrows, fixture construction, formatting, clippy, cross-target checks, and GPU execution remain correctly deferred to the implementation plan and real toolchain.