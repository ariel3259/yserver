# Phase C.0 stage 2c-i debt — guard-clause proof, husk accounting, reset posture

**Status:** design, 2026-09-15. A small prelude stage between the accepted
stage 2c-i and stage 2c-ii. Implementation plan to follow.

## 1. Why this stage exists

Stage 2c-i was accepted on 2026-09-14 (`e00e5971`) with a recorded trade, not
a clean bill of health. Mutation sampling of Tasks 5–7 never converged: two
batteries, seventeen mutations, seven survivors, against fourteen mutations
across Tasks 1–4 and 8–10 that produced one survivor — and that one
(`record_device_barrier`'s device check) turned out to be redundancy with a
second guard rather than a coverage gap, with the property still proven
end to end. The user's call was to close the
known findings and accept, with a **systematic pass** to follow rather than a
third round of sampling.

This stage is that pass, plus the two carried findings that belong with it. It
is deliberately separate from 2c-ii: 2c-ii is *bounded intents and admission*
per the stage-2c design, and mixing 2c-i debt into it would grow a plan whose
size is the measured predictor of defect density here (14 tasks → 2 blocking,
21 → 24, 23 → 26).

## 2. The census — measurement, not estimate

The pass is defined by a measurement taken on 2026-09-15 at `b96e4e9e`, not by
a prose description of "improving coverage".

Every guard clause in `resources/{commit,gpu,transport}.rs` was enumerated and
neutralised one at a time — an `if` guard's condition forced to `false`, a
refusing match arm turned into its accepting counterpart — and the full
`c0_2ci` suite (including hardware) run against each.

| | |
| --- | --- |
| Guard sites | 33 |
| **Caught** — deleting the guard fails at least one test | **17** |
| **Survivors** — deleting the guard leaves the suite green | **16** |

Six of the 33 could not be neutralised generically (`if let` guards whose body
binds the matched value) and were mutated by hand; two of those were caught,
four survived. A site that does not compile is never counted as a result.

**48% of the guard surface in these three files is unproven.** That is the
fact this stage exists to change, and it confirms that the earlier
non-convergence was a property of the surface rather than of the sampling.

### 2.1. The survivors, by mechanism

Guards are identified by **function and invariant**. Line numbers are given as
a dated convenience at `b96e4e9e` and are **not authoritative** — they move as
soon as the first item lands.

**A. Owner handover entry — five survivors, the whole path.**
`TransportGate::issue_handover_permit` (both guards) and
`TransportGate::publish_owner` (all three). This pair *is* the transition into
Owner that R7 governs, and not one of its guards has decisive coverage:
quiescing state, outstanding owner writes, and the permit's device/incarnation
binding can each be deleted with the suite staying green. An entire unproven
path is where defects hide, which is why this group is called out first.

**B. Error propagation — six survivors across two files.**
`CommitResourceConsumer::consume` swallowing a recovered transition error
(~263) and failing to re-arm the direct role (~285); `on_available` swallowing
its transition error in both halves (~437 releasing, ~490 rejected);
`gpu.rs`'s `cancel_pre_submit_batch` (~383) and `freeze_uncertain_batch` (~406)
error arms. `on_available` can swallow a transition error and return `Ok` with
admission already closed, and nothing notices — a failure reported as success,
which is the class R9 exists to prevent.

**C. Releasability and uniqueness — three survivors.**
`is_resource_releasable`'s `source` and `fallback` branches (~510, ~515) — the
`allocations` branch is covered and F14-M1 closed the `kms_obligations` one, so
these are the two siblings left; and `register_commit_dependencies`'s
`validate_unique` over the **new** members (~593), where the same check over
the old members is covered.

**D. Transport edges — two survivors.**
`authorize_write`'s `Closed` arm (~356), where the `Quiescing` arm is covered;
and `consume_owner_write`'s non-Owner state check (~402).

## 3. The three items that are not census survivors

### 3.1. F13c-m1 — husk accounting must not be skippable

`OutputScanout::detach_managed_entries(Option<&mut DrmCleanupRegistry>)` can be
called with `None`, dropping managed leases with no accounting, and the
production route (`drain_scanout_pool_at`, reached from suspend) already takes
that arm. It is inert only while R8 holds; once managed entries can exist in
production, the fd-family barrier's inventory stops following the husk and the
barrier can never mint again.

Making the registry mandatory is rejected: `PlatformBackend` has none in scope,
and plumbing one in is activation work R8 excludes. Instead the function
**returns what it released**, as a `#[must_use]` receipt discharged either by
handing it to a registry or by explicitly asserting it was empty. This matches
the vocabulary already in use — `FileFamilyClosed` sealed with `_private: ()`,
`DeviceBarrier` taking its proof by value.

**Its acceptance criterion is compile-time, not test-time:** with clippy at
`-D warnings`, ignoring the receipt fails the build. That is stronger than any
mutation, and it is the only item in this stage with that property.

### 3.2. F13c-m2 — silent underflow

`unregister_pool_husk`'s `saturating_sub(1)` absorbs an unregister with nothing
registered. A `debug_assert!` before the subtraction, and a test that trips it.
This item may shrink on its own once 3.1 lands: a count that travels as a
receipt is structurally harder to underflow.

### 3.3. R9 versus the server reset — a tripwire, declared as one

Upstream's `feat #148` (merged here at `36ba48d5`) brought server reset.
`force_destroy_all_clients` frees backend resources through
`process_disconnect` and the `host_xid_still_referenced` orphan gate:
**it frees on a reference gate, synchronously.** Stage 2c-i's model frees on a
correlated proof (R9) and retains failed or timed-out tickets for teardown.

The concrete path is `force_destroy_all_clients` → `process_disconnect` →
`backend.free_pixmap` → `store.decref` → `destroy_now` → `Storage::destroy`,
whose `Managed` arm drops the lease and lets the service decide destruction on
its own terms via `can_destroy`. Today that arm is unreachable in production —
R8 keeps `resource_service: None`, so nothing is ever `Managed` there and the
chain always takes the `Legacy` arm.

**The tension runs the opposite way from the obvious reading.** Our model does
not drop resources undischarged under a reset; it *gates* them. An entry with
an outstanding `KmsRelease` survives until its proof lands — and the reset
design states that nothing client-created survives, by design, because a reset
erases the session. So proof-gating and erasure contradict each other, and the
open question is which wins. That question belongs to whichever stage first
makes managed entries reachable in production, which per the stage-2c design's
section 6 is **stages 3/4** — not 2c-ii and not 2c-iii, both of which keep the
production route `Legacy` with readiness closed.

Because there is no mechanism yet, this item cannot have a
delete-the-mechanism mutation, and pretending otherwise would be the vacuous
test this project keeps punishing. It is a **tripwire**: a test that constructs
a backend with an installed service holding an entry with a pending
obligation, drives the reset's forced teardown against it, and asserts the
entry's fate. It passes trivially today. Its job is to fail on the day someone
wires the reset to the ledger, and force the question to be answered then.

**Its acceptance criterion is a mutation toward the future:** add the wiring
that does not yet exist — make the forced teardown drop a managed lease — and
the test must fail. A tripwire that does not fail under that is observing
nothing and must not be kept.

## 4. Evidence and review

The stage delivers: the census as a committed tool under `tools/` with its
dated baseline run; decisive coverage for all sixteen survivors, each
recording the mutation that must fail a named test — how many tests that
takes, and where they live, is the implementer's call, and one test may
cover more than one guard; the F13c-m1 receipt; the F13c-m2 assertion and its
test; and the R9 tripwire with its future-mutation criterion written beside it.

Review is per item — the named mutation re-run by the reviewer, addressed **by
line number and confirmed to have compiled** — and per group as groups land,
rather than once at the end. The global criterion is the census re-run at
**zero survivors** in the three files, with the six hand-mutated sites named
explicitly as outside the tool's reach.

That global criterion has a property worth stating: **it self-reports
incompleteness.** If an item cannot be closed honestly and the session takes an
F8 stop, no one needs to trust the report — the census shows the survivor and
the stage does not reach zero.

**Risk, stated because it can break this stage's frame.** This is declared a
tests-only stage apart from 3.1 and 3.2. That is a hypothesis, not a fact:
groups A and B are entire unproven paths, and unproven paths are where real
defects live. If a test cannot be made to pass because the code is wrong, that
is an F8 stop — report it, leave the item open, and decide the mechanism fix
separately with its own review. It is not fixed quietly inside a test session.
The pressure to make a test go green by touching the mechanism is the original
failure of this whole round.

**Out of scope:** admission and bounded intents (2c-ii); F13b-D1, the
dispatched `CommitResources` carrying no present-pin leases by value (2c-iii,
per the F-13b review — three later summary lines said 2c-ii and were wrong);
and any activation whatsoever.

**Gate:** `cargo +nightly fmt`; `cargo clippy --all-targets -- -D warnings`;
`cargo test -p yserver --lib c0_2ci` plus its hardware run; the twelve-run
flake loop; the full workspace suite, which now matters because XDMCP, TCP
transport and server reset are on the branch; and the gnu/musl/freebsd target
checks, because 3.1 touches `drm_cleanup.rs`.
