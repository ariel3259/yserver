# Copied scanout route design — codex review, round 3

**Target:** `docs/superpowers/specs/2026-09-22-phase-c0-stage-2c-iii-copied-route-design.md`
revision 3 (`faceac03`), against the 2c-iii conversion design as the passed parent.
**Prior:** round 2 (`../findings/2026-09-22-stage-2c-iii-copied-route-design-review-round2.md`).

**Reviewer:** `codex exec --sandbox read-only`, single pass
**Instrument:** `docs/superpowers/review/` @ `da807b70`;
model `gpt-5.6-sol`; reasoning effort `xhigh`; `codex-cli 0.155.1`.
Counts are comparable only to other reviews citing this same instrument SHA.
Coverage COMPLETE FOR DECLARED SCOPE, 24/24 excerpts used; `CompletionPoller`
internals, teardown/device-loss behaviour, transport encoding and the delegated
`FenceTicket`/fixture choices are recorded as unassessed.

**Author verification (2026-09-22), every finding checked against the tree:**

- **B-1 — CONFIRMED.** `service_completions` returns
  `Result<Vec<AllocationKey>, ResourceError>` (`resources/mod.rs:359`): generic
  keys and a service-wide error, neither of which says anything about one
  generation's obligation. The composed precedent logs a service failure and
  offers anyway (`scene.rs:3965`). Revision 3 required the destination's
  obligation to retire before the offer but never said how the consumer learns
  it. Fixed by existing API rather than new machinery: new CP-2a makes the
  generation hold the receipt `(destination key, obligation id)` and promote
  only on `has_pending_obligation` false (`resources/mod.rs:290`) **and**
  `is_frozen` false (`resources/mod.rs:911`).
- **M-1 — CONFIRMED, and the answer already existed.** `quarantine_gpu_batch`
  takes the keys it freezes from `batch.obligation` and `batch.read_obligation`
  (`resources/mod.rs:1563`), so revision 3's ticketless `possibly_dispatched`
  batch would have frozen **no destination key at all**. The project's own
  helper `abandon_unsubmitted_batch` (`resources/gpu.rs:395`) already implements
  both dispositions and the 2c-i rule that an unknown submission retains its
  reservation and closes the transport. CP-4b now uses it.
- **M-2 — CONFIRMED, and it invalidates a mutation revision 3 had asserted.**
  Selection scans the destination pool for `BoPhase::Free`
  (`platform.rs:6337`) and transitions the chosen bo to `Recording` before
  returning (`platform.rs:6374`), with B-13's comment saying exactly why. The
  source is paired by `bo_idx`, so no later tick can select it while the
  destination is `Recording`, and revision 3's "insert a yield" mutation could
  not have failed. CP-4c now names the paired phase as the authority and
  mutates that instead.
- **M-3 — CONFIRMED by reading section 6 itself.** The cross-device criterion
  was paired only with CP-8's forced-legacy mutation, which is a different
  criterion and can fail on route gating without the sink copy ever running.
  Fixed: the run records the distinct renderer and sink identities and carries a
  mutation that misroutes the copy off the sink's device.
- **m-1 — CONFIRMED, author's slip.** The status line still said revision 2
  while the body described revision 3. Fixed on the way to revision 4.

**Pattern worth recording after three rounds.** Every blocking and major finding
in rounds 2 and 3 was repaired by something the tree already had —
`prepare_retirement_batch`, `abandon_unsubmitted_batch`, `has_pending_obligation`,
the paired `BoPhase`. The recurring author error was not bad judgement about what
should happen; it was inventing a mechanism without first asking whether the
resource service already exposed one. The plan should be written with that order
reversed.

Revision 4 incorporates all five.

---

## Verdict

**1 blocking, 3 major, 1 minor**

Coverage: COMPLETE FOR DECLARED SCOPE

This is a design-review result only; it does not claim compilation, test success, or implementation approval.

## Incorporation audit

| Prior finding | Status | Assessment |
|---|---|---|
| B-1 — A→B handoff lacked a closed ownership protocol | **TRADED** | Revision 3 correctly discards the nonexistent lease-transfer premise, retires A before reserving B, and moves B’s registrations before submission. However, CP-4b’s new uncertain-dispatch disposition does not preserve ownership of B’s prepared destination obligation; see M-1. |
| M-1 — Post-submit wake-registration failure lacked an owner | **APPLIED** | Section 3.3 now displaces the generation, forbids an offer, removes partial waiter state, and leaves the batch with the resource service for retirement or quarantine ([design:247](</home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-22-phase-c0-stage-2c-iii-copied-route-design.md:247>)). |
| M-2 — Evidence could not prove the destination obligation | **APPLIED** | The hardware evidence now requires registration under the destination’s own key followed by retirement, with separate omission and miskey mutations ([design:313](</home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-22-phase-c0-stage-2c-iii-copied-route-design.md:313>), [design:365](</home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-22-phase-c0-stage-2c-iii-copied-route-design.md:365>)). |

## Findings

### Blocking

#### B-1 — No event contract carries B’s authoritative retirement to the offer transition

CP-1/CP-2 require the specific destination obligation to retire before `Desired` or an offer, and explicitly reject fence readability and generic availability as authority ([design:129](</home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-22-phase-c0-stage-2c-iii-copied-route-design.md:129>), [design:247](</home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-22-phase-c0-stage-2c-iii-copied-route-design.md:247>)). That is required by the parent’s rule that readiness includes finished producer waits ([spec:307](</home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-19-phase-c0-stage-2c-iii-conversion-design.md:307>)).

No corresponding information-delivery contract is defined. `service_completions` returns generic allocation keys and may return an error caused by any expired or failed batch ([resources/mod.rs:359](</home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/resources/mod.rs:359>)). The composed precedent merely calls it, logs failure, and offers unconditionally ([scene.rs:3965](</home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/scene.rs:3965>)). Section 9 delegates poller placement, but not how the consumer learns that B’s exact `(destination key, obligation id)` retired.

Concrete failure: B’s sync file wakes; servicing quarantines B because validation fails, or returns an unrelated availability edge/error; a handler following the cited composed shape still transitions and offers a destination whose write obligation did not retire.

Smallest correction: define a generation-correlated retirement receipt/query, carrying at least destination key plus obligation/batch identity. `Desired` and the offer must consume explicit successful retirement of that identity. Add a mutation where B’s fence is readable but B’s obligation remains pending or is quarantined; no offer may result.

### Major

#### M-1 — CP-4b’s ticketless batch cannot own or quarantine the prepared destination obligation

CP-4b says uncertain dispatch registers a batch with no ticket and `possibly_dispatched` set ([design:201](</home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-22-phase-c0-stage-2c-iii-copied-route-design.md:201>)). But `CoreRetirementBatch` retains prepared write entries only through `Option<GpuObligation>`; `possibly_dispatched` carries no entries ([gpu.rs:144](</home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/resources/gpu.rs:144>)). On quarantine, destination keys are obtained only from that optional obligation, while the separate read obligation covers only the source ([resources/mod.rs:1563](</home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/resources/mod.rs:1563>)).

Thus the destination obligation registered at CP-4a step 2 loses its batch-level identity on an uncertain result. The existing uncertain-dispatch helper instead closes the transport and freezes the prepared entries directly ([gpu.rs:388](</home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/resources/gpu.rs:388>)).

Smallest correction: retain prepared write entries independently of ticket binding and quarantine them, or use the established close-and-freeze path. Evidence must inspect the destination, not merely `possibly_dispatched`.

#### M-2 — CP-4c’s claimed exclusion mechanism is not the mechanism the tree runs

CP-4c claims a later tick could reuse the source and that only no-yield execution prevents it ([design:209](</home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-22-phase-c0-stage-2c-iii-copied-route-design.md:209>)). In the existing allocator, candidates require the paired destination BO to be `Free` ([platform.rs:6329](</home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/platform.rs:6329>)), and selection transitions it to `Recording` before returning ([platform.rs:6367](</home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/platform.rs:6367>)). A later tick therefore cannot select that paired source merely because a yield occurs.

Consequently, the section 8.2 “insert a yield” mutation need not fail and cannot prove the claimed invariant.

Smallest correction: identify the authoritative exclusion mechanism—paired BO phase ownership or an explicit source lease—and mutate that mechanism. If non-interleaving remains required for another source consumer, enumerate that consumer and schedule it during the yielded window.

#### M-3 — The hardware mutation does not test the cross-device criterion

The cross-device criterion is paired only with CP-8’s forced-legacy mutation ([design:381](</home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-22-phase-c0-stage-2c-iii-copied-route-design.md:381>)). That mutation tests Owner/Legacy exclusivity, already a separate criterion, rather than whether renderer-A output is read and copied on sink B. It can fail because of route gating without exercising cross-device copy correctness. The parent separately requires device isolation and per-site exclusivity ([spec:496](</home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-19-phase-c0-stage-2c-iii-conversion-design.md:496>)).

Smallest correction: record distinct renderer and sink identities and add a mutation that bypasses or misroutes the sink copy/context; the cross-device test must catch that independently of CP-8.

### Minor

#### m-1 — Revision provenance contradicts itself

The status still identifies revision 2 ([design:3](</home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-22-phase-c0-stage-2c-iii-copied-route-design.md:3>)), while lines 16–25 claim revision 3 incorporation. This makes acceptance and subsequent plan reviews ambiguous. Update the status to revision 3.

## Coverage and implementation checks

- Incorporation: audited all three prior-review rows.
- Architecture/contracts: checked A/B ownership, service-to-offer delivery, copied selection, and event-loop waiter behavior.
- Safety/failure: checked preparation unwind, uncertain dispatch, quarantine ownership, leases, and CP-4c non-interleaving.
- Compliance/evidence: checked parent readiness, device isolation, exclusivity, hardware evidence, and mutations.

Excerpts used: **24/24** beyond the target and prior review. The budget was exhausted. CompletionPoller internals, full teardown/device-loss behavior, transport encoding, and delegated `FenceTicket` and fixture construction remain unassessed and are not deemed sound.

Rust signatures, borrows, builds, formatting, clippy, cross-target checks, tests, and GPU execution remain deferred to the implementation plan and real toolchain.