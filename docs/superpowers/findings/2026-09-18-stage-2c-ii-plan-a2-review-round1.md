## Verdict

**2 blocking, 3 major, 0 minor**

Coverage: **COMPLETE FOR DECLARED SCOPE**

**Target:** plan A2 revision 1 (`f0c1521c`), against spec revision 3 and A1's implemented API.

**Reviewer:** `codex exec --sandbox read-only`, single pass
**Instrument:** `docs/superpowers/review/` @ `69c6d6e2`;
model `gpt-5.6-sol`; reasoning effort `xhigh`; `codex-cli 0.154.0`.
Counts are comparable only to other reviews citing this same instrument SHA.

**Recorded usage:** 65,111 tokens (exit 0). 12/12 excerpts.

**Author verification (2026-09-18):**

- **B-1 — CONFIRMED.** `begin_with_context` (`device.rs:1322`) takes `Submitted<R>` by value and returns `Err` before installing the record on the legacy transport, `next_correlation`, build, `validate_completion_context` and `slot.reserve`; no `DispatchError` carries the ledger, so a refused `begin` drops it (and a dropped `RoleReservation` closes admission). Fixed with `begin_with_ledger`: the builder runs only after the last refusal point and is returned uncalled.
- **B-2 — CONFIRMED.** `set_direct_successor` returns `UnflipPending` (`intents.rs`); `request_unflip` returns the displaced descriptor while `scanout_m2.queued_successor` and its role would remain. Fixed: offer refused before the seam; unflip terminalizes the exact frame (step 1 of `managed_handle_direct_unflip`, factored out).
- **M-1 — CONFIRMED, fixed as an extension of approved decision 4.** `scanout_direct_eligible`'s inputs are computed inline in `try_present_direct` (VT, clock epoch, cursor mode, root coverage) and cannot be satisfied in a Vulkan-less fixture; eligibility enters through `AdmissionSource::direct_eligible`, false invalidates, the real predicate is 2c-iii's.
- **M-2 — CONFIRMED.** Fixed with an operation trace.
- **M-3 — CONFIRMED.** Fixed with non-empty-current refusal scenarios for both shapes.

This is a bounded design-review result, not a claim that the plan compiles, tests pass, or implementation is approved.

## Incorporation audit

| Prior finding | Status | Assessment |
|---|---|---|
| — | Skipped | No prior review exists, as stated in the brief. |

## Findings

### Blocking

#### B-1 — The proposed post-`begin` bind cannot put direct resources into the owner’s ledger

The plan requires `prepare → begin → bind`: `begin` allocates the `CommitId`, then `managed_bind_direct_dispatch` stamps the prepared resources and returns them ([plan](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-18-phase-c0-stage-2c-ii-plan-a2-conductor.md:96), lines 96–109). But Task 3 simultaneously requires those resources to be the ledger’s `new` value passed to `begin` ([plan](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-18-phase-c0-stage-2c-ii-plan-a2-conductor.md:221), lines 221–230).

The actual owner API consumes `Submitted<R>` when `begin_with_context` is called and installs it directly into the live record ([device.rs](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/owner/device.rs:1322), lines 1322–1354); `begin` has the same by-value ledger contract ([device.rs](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/owner/device.rs:1412), lines 1412–1418). There is no opportunity afterward to insert the resources returned by `bind`.

Concrete failure: prepare moves `Successor` toward `Submitted`; either the conductor gives the prepared resources to `begin`, losing the value required by post-`begin` bind, or it calls `begin` without them, leaving the owner’s live ledger unable to return them on refusal/retirement. On a `begin` error, the current API also does not return the consumed ledger, so the promised restoration of `take_current()` resources is not implementable for every refusal. This violates the by-value resource and abort contracts in spec §6 and §7 ([spec](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-18-phase-c0-stage-2c-ii-admission-design.md:265), lines 265–293; 303–321).

Smallest correction: design an owner-side two-stage begin permit, or a ledger-builder API, that exposes the allocated `CommitId` before ledger ownership transfers and returns/retains all resources on every pre-install failure. Then rewrite prepare/bind/undo and the begin-refusal tests around that ownership boundary.

#### B-2 — Direct descriptor and managed frame are not transacted atomically across unflip

A1’s actual `set_direct_successor` can reject while an unflip is pending ([intents.rs](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/owner/admission/intents.rs:75), lines 75–100), and `request_unflip` removes and returns the existing direct descriptor ([intents.rs](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/owner/admission/intents.rs:115), lines 115–130).

The plan nevertheless specifies direct offer as `managed_prepare_direct_candidate` plus `set_direct_successor`, without rollback for an A1 rejection, while `admission_request_unflip` has no contract for consuming the displaced descriptor and terminalizing its matching frame/reservation ([plan](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-18-phase-c0-stage-2c-ii-plan-a2-conductor.md:154), lines 154–180).

Concrete failures:

- If a direct offer mutates the managed seam before `set_direct_successor` returns `UnflipPending`, a frame and `Successor` charge remain without a descriptor.
- If unflip is requested after a direct offer, A1 removes the descriptor while `scanout_m2.queued_successor` and its reservation remain. The existing never-submitted helper only releases pins/events; reservation discharge is a separate responsibility ([backend.rs](/home/ariel_santangelo/Projects/yserver-phase-b/crates/yserver/src/kms/render/backend.rs:2552), lines 2552–2564).

This violates the spec’s barrier-displacement and never-submitted disposition requirements ([spec](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-18-phase-c0-stage-2c-ii-admission-design.md:108), lines 108–128; [spec](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-18-phase-c0-stage-2c-ii-admission-design.md:363), lines 363–366).

Smallest correction: specify a two-sided transaction for direct offers, including rollback on every `AdmissionError`, and require `admission_request_unflip` to correlate the returned generation with the exact queued frame, discharge its reservation, and run the frame through the never-submitted path. Add both ordering scenarios as tests.

### Major

#### M-1 — The snapshot has no contract for current paint-chain eligibility

Spec §4 requires live `scanout_direct_eligible` status in addition to matching layout/topology generations ([spec](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-18-phase-c0-stage-2c-ii-admission-design.md:144), lines 144–176). The plan instead defines direct eligibility as only the decider’s generation comparison ([plan](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-18-phase-c0-stage-2c-ii-plan-a2-conductor.md:173), lines 173–181). `AdmissionSource` carries producer waits but no current paint-chain eligibility, and direct offer names only candidate preparation plus descriptor insertion.

Concrete scenario: a framebuffer-valid candidate is offered while the resolved chain has a border; producer readiness is `Ready` and generations match, so the plan permits dispatch despite spec-ineligibility. The layout-change test covers later invalidation, not initial or independently recomputed eligibility.

Correction: make current direct eligibility an explicit snapshot input or require the conductor to query the existing eligibility seam on every snapshot; false must atomically invalidate and terminalize the successor. Add an initially-ineligible case and a mutation that forces eligibility true.

#### M-2 — The retirement test cannot distinguish enqueue-before-admission

The named test observes C dispatched after `route_owner_event` returns, then drains and observes A followed by B ([plan](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-18-phase-c0-stage-2c-ii-plan-a2-conductor.md:248), lines 248–265).

Mutation N11—run admission after resource consumption but before enqueuing A/B—produces the same observations: C is dispatched, then A and B are enqueued, and the later drain returns A then B. Recording the dispatch observation before the drain proves publication-last, not enqueue-before-admission.

Correction: instrument operation order, or have the admission snapshot/source assert that A’s completion and B’s deferred `Skip` are already queued when readiness is queried.

#### M-3 — Refusal tests do not require a nonempty `ResourcesStillCurrent` path

The refusal invariant requires consuming every returned event and restoring old current resources ([plan](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/plans/2026-09-18-phase-c0-stage-2c-ii-plan-a2-conductor.md:227), lines 227–239; [spec](/home/ariel_santangelo/Projects/yserver-phase-b/docs/superpowers/specs/2026-09-18-phase-c0-stage-2c-ii-admission-design.md:303), lines 303–323). The real Reaped scenario does not require an existing current resource, and the constructed six-cause matrix does not state that its event payload contains both nonempty released-new and still-current-old resources.

A handler that processes `ResourcesReleased` but drops or ignores `ResourcesStillCurrent` can therefore satisfy every stated assertion while losing current ownership on a replacement refusal.

Correction: require a refusal fixture that displaces nonempty current state and assert exact restoration and role occupancy; exercise both direct and composed ledger shapes, or prove they share one tested event-consumption function.

### Minor

None.

## Coverage and implementation checks

- Incorporation: skipped because no prior review exists.
- Architecture/contracts: checked plan-to-A1 ownership, direct/unflip correlation, eligibility inputs, and retirement sequencing.
- Safety/failure semantics: traced by-value ownership through direct preparation, A1 displacement, `begin`, abort, and never-submitted disposal.
- Spec/verification: checked relevant §4, §6, §7, and §10.2 requirements and N1–N13 evidence claims.

Excerpts used: **12/12**. Verified ground included the relevant spec sections, A1 intent/token APIs, managed candidate/dispatch/disposal contracts, and actual owner `begin` ownership. Detailed bodies of `route_owner_event`, `CommitResourceConsumer::consume`, `send_on`/`retire_live`, `record_host_call_events`, and `transport_gate` were not assessed after the excerpt budget was exhausted and are not claimed sound. Builds, tests, mutation runs, ABI checks, and compiler-level API validation remain deferred to implementation.