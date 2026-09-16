# Phase C.0 stage 2c-i debt — refusal proof, husk identity, reset boundary

**Status:** design, revision 2 (2026-09-16). Revision 1 (`0a96e329`,
`8f52e131`) was reviewed by codex in round 1
(`docs/superpowers/findings/2026-09-16-stage-2c-i-debt-design-review-round1.md`:
3 blocking, 1 major, all verified). This revision incorporates that review and
the author's refusal inventory and `mod.rs` census
(`docs/superpowers/findings/2026-09-16-stage-2c-i-debt-refusal-inventory.md`).
Implementation plan to follow.

## 1. Why this stage exists

Stage 2c-i was accepted on 2026-09-14 (`e00e5971`) with a recorded trade, not
a clean bill of health: mutation sampling of its Tasks 5–7 surface never
converged. This stage is the deferred systematic pass. It sits between 2c-i
and 2c-ii so that 2c-ii stays what the stage-2c design scoped it as — bounded
intents and admission — and does not grow by absorbing debt.

## 2. The measurement

Every refusal guard in the resource service —
`resources/{mod,commit,gpu,transport}.rs` — was neutralised one at a time (an
`if` guard's condition forced to `false`, a refusing match arm turned into its
accepting counterpart) and the full `c0_2ci` suite, hardware included, run
against each. A site whose mutation does not compile is never counted.

| File | Sites | Caught | Survivors |
| --- | --- | --- | --- |
| `mod.rs` | 34 | 15 | 19 |
| `transport.rs` | 18 | 11 | 7 |
| `commit.rs` | 12 | 5 | 7 |
| `gpu.rs` | 3 | 1 | 2 |
| **Total** | **67** | **32** | **35** |

**52% of the resource-service refusal surface is unproven.**

The first census (revision 1) covered only `commit`, `gpu` and `transport`.
`mod.rs` — the ledger core and most of the GPU batch state machine, with more
refusal points than the other three files together — was left out because the
file list was taken from an earlier recommendation without checking where the
guards live. That is corrected here.

### 2.1. What the measurement does not cover

Stated so that "zero survivors" is never read as more than it is:

- **Absent guards.** A census mutates guards that exist. A refusal the
  contract requires and the code lacks is invisible to it; section 4 covers
  those, found by reading the contracts.
- **Other control-flow forms.** `?`-propagated refusals, refusals returned
  from helpers, assertions and state-transition APIs are not enumerated.
- **Oracle strength (revision 1).** The first census counted a mutation as
  caught when *any* test failed, so one killed by an incidental panic was
  indistinguishable from one killed by the intended assertion. Section 5.1
  closes this for the work this stage adds.

### 2.2. The pattern behind the survivors

The same shape appeared three times in two days of review: `cancel` proven on
one of its three paths, the `frozen` check proven on one of
`validate_gpu_batch`'s three check sets, and the identity check proven at two
of the ledger's twelve entry points — the two by a single test that was never
extended to the rest. **This stage's tests proved one instance of a guard and
left its siblings unproven**, and sampling found the siblings one at a time.

So every invariant below is stated **per family**: all siblings, and each
sibling's deletion failing a named test.

## 3. Session 1 — tests only

Guards are identified by **function and invariant**. Line numbers would move as
soon as the first item lands, so none are given as identity.

### 3.0. The census, as a committed tool

The census becomes a tool under `tools/`, run at the start of the session to
record a dated baseline, and at the end as the acceptance measurement. It is
test infrastructure, not a production change, which is why it opens this
session rather than the mechanism session: session 1's own acceptance
criterion depends on it.

The oracle (section 5.1) requires knowing which test is meant to kill each
guard. That association lives **beside each test**, as a tag in its doc
comment naming the family and function it proves; the tool reads the tags. No
central list exists to drift.

The tool mutates source files and runs cargo. It is a developer tool, not a CI
step, and must restore every file it touches even on failure.

### 3.1. The families

**A. Owner handover entry** (`transport.rs`, 5). `issue_handover_permit`
refuses unless the gate is `Quiescing` and no owner write grant — helper grants
included, which is how helper permissions are modelled — is outstanding.
`publish_owner` refuses a permit bound to another device or incarnation, and
refuses unless `Quiescing` with no outstanding grant. This is the transition
into Owner that R7 governs, and none of its guards was proven.

**B. Transport edges** (`transport.rs`, 2). `authorize_write` refuses every
writer class while `Closed` (its `Quiescing` arm is already proven);
`consume_owner_write` refuses unless the gate is `Owner`.

**C. Consumer error propagation** (`commit.rs`, 4). `consume` propagates a
recovered transition error and re-arms the direct role; `on_available` returns
its transition error, in both its releasing and its rejected halves, rather
than `Ok` with admission already closed. A failure reported as success is the
class R9 exists to prevent.

**D. Releasability and uniqueness** (`commit.rs`, 3). `is_resource_releasable`
refuses while the `source` or the `fallback` present-pin allocation is not
releasable — the siblings of the allocation and `kms_obligations` branches
already proven. `register_commit_dependencies` refuses duplicate members in the
**new** set, as it already provably does for the old set.

**E. Cross-incarnation isolation** (`mod.rs`, 10). Every ledger entry point
that accepts an `AllocationKey` or a proof refuses one from another device or
incarnation: `register`, `freeze`, `cancel`, `validate_proof_target`,
`record_kms_discharged`, `apply_teardown_release` (its key check and its proof
incarnation check), and `validate_gpu_batch` in all three of its check sets.
Only `reserve` and `apply_validated_proof` are proven today. R9's "stale
evidence cannot release a newer allocation" rests on this family.

**F. Read-obligation validation** (`mod.rs`, 4). `validate_gpu_batch` refuses a
read obligation whose source entry, or whose staging entry, is frozen or holds
no matching pending obligation. Together with family E's two read-path identity
checks, this is the whole read-obligation path, of which no guard was proven.

**G. Exhaustion** (`mod.rs`, 3). `adopt_unchecked`, `reserve` and `register`
each refuse once the service is exhausted.

**H. File-owned adoption** (`mod.rs`, 1). `adopt` refuses a payload carrying a
live file-owned alias; such a payload must go through `adopt_with_registry`.

**I. Teardown precondition** (`mod.rs`, 1). `apply_teardown_release` refuses an
entry that is not frozen.

That is 33 of the 35 survivors. The other two are not test work — see 4.2.

## 4. Session 2 — mechanism changes

This session changes production code. It is kept apart from session 1 on
purpose: if a session-1 test exposes a real defect, there must be no mechanism
change in the same session to fix it into quietly.

### 4.1. Husk accounting bound to identity (round-1 B-2, B-3)

Today `register_pool_husk` is `+= 1` and `unregister_pool_husk` is
`saturating_sub(1)` on an unkeyed scalar, and
`detach_managed_entries(Option<&mut DrmCleanupRegistry>)` can be called with
`None` — which the production route `drain_scanout_pool_at`, reached from
suspend, already does. Three defects follow: accounting can be skipped; a
spurious unregister consumes another husk's count, so the registry can certify
zero aliases while a husk is live; and nothing correlates an unregister with
the registry that holds the registration.

Revision 1 proposed a `#[must_use]` receipt and a `debug_assert!`. Both are
withdrawn. `#[must_use]` rejects a bare expression statement but accepts
`let _ =` and `drop(...)` — verified by compiling it — so it does not make
accounting compulsory. A pre-decrement `debug_assert!(count > 0)` passes on a
spurious unregister while any other husk is live, and release builds drop it.

The required shape: **registering a husk yields a registration token bound to
its device and incarnation; unregistering consumes that token by value; the
registry validates it against its own identity.** An unknown or foreign token
fails closed — the registry refuses to mint `FileFamilyClosed` — rather than
saturating. A token dropped without being consumed also fails closed. This
follows the house precedent of the lost-role-token rule, where a
`RoleReservation` dropped undischarged closes admission.

### 4.2. Swallowed GPU errors, and the transport close they owe (inventory AB-3)

The two `gpu.rs` census survivors — the error arms of `cancel_pre_submit_batch`
and `freeze_uncertain_batch` — cannot be proven by a test, because their only
production callers, in the managed scanout write path of `scene.rs`, discard
both results with `let _ =`. A test of either function alone would pass while
the real path still ignores the failure. **The defect is the swallowed error.**

Fixing it requires deciding what the caller does with the error, and the 2c-i
design's section 4 already says: "Unknown submission retains its reservation
**and closes the affected transport** until its specified recovery proof
arrives." The retention half exists; the close half exists nowhere. So this
item propagates the failure and closes the gate when one is installed. That is
inert in production under R8, where no gate is installed.

Acceptance: deleting the propagation, or the close, fails a named test that
drives the real `scene.rs` path.

### 4.3. The reset boundary (round-1 B-1)

Upstream's server reset (`#148`, merged at `36ba48d5`; left unconditional by
`#149`, merged at `50d86524`) frees backend objects through
`force_destroy_all_clients` → `process_disconnect` → `backend.free_pixmap` →
`store.decref` → `destroy_now` → `Storage::destroy`.

Revision 1 framed this as a contradiction between the reset's promise that
nothing client-created survives and proof-gated release, left "which wins"
open, and proposed a tripwire that would fail if forced teardown dropped a
managed lease. **That was wrong.** The 2c-i design already separates two
levels: "Cache removal, drawable destruction and pool replacement detach
logical owners; they cannot destroy backing allocations retained by a live or
quarantined lease", and stage 3 transfers those retained owners into its
teardown supervision. The reset's erasure is of protocol-visible state —
resources, atoms, selections, grabs, properties — and a reset "never touches
KMS, Vulkan". A proof-bearing backing that outlives its XID does not survive
in any sense the reset promises against. And the proposed tripwire would have
fired on correct code: `Storage::destroy`'s `Managed` arm drops that very
`Retain` lease today, by design.

The item is therefore a real test of the contract 2c-i already owns, driven
through the reset's own entry point. With a service installed and a managed
drawable holding a pending obligation, after `force_destroy_all_clients`:

- the old XID no longer resolves — identity is erased;
- the backing entry is still rooted in the service — physical release is gated;
- once the obligation's proof is applied, the entry is destroyed.

Its mutation: make the forced teardown physically destroy a backing whose
obligation is still pending. **Not** "drop the lease" — that is required.

The transfer of those retained backings into a teardown supervisor is
**stage 3**, and this test does not claim it. If the reset entry point cannot be
driven against a managed drawable in a test without new production wiring, that
is an F8 stop to report, not a reason to add the wiring.

## 5. Evidence and review

### 5.1. Acceptance

**Session 1:** each family's invariant holds with every sibling proven. The
census tool, re-run, reports zero survivors over the enumerated baseline of
section 2 **and** every killing test is the one tagged for that guard — a guard
killed only by an untagged or unrelated test does not count.

**Session 2:** 4.1's token fails closed on a foreign, unknown or dropped token,
each proven by a named test; 4.2 and 4.3 as stated in their sections.

Both criteria are measured, so they **self-report incompleteness**: an item left
open by an honest F8 stop shows up as a survivor or a failing criterion, and no
one has to rely on the report confessing it.

### 5.2. Review

Per item: the reviewer re-runs the named mutation, addressed by line number at
review time and confirmed to have compiled. Per family, as families land. The
global census re-run closes session 1.

### 5.3. Risk

Families A, C, E and F were whole unproven paths, and unproven paths are where
real defects hide. If a session-1 test cannot pass because the code is wrong,
that is an F8 stop: report it, leave the family open, and decide the fix
separately with its own review. The session split exists so that this is a
boundary, not a temptation.

## 6. Out of scope

- **Absent mechanisms left to stages 3/4** (inventory AB-1, AB-2).
  `WriterCoverageProof` is an empty token whose test constructor demands no
  coverage, and the `RecipientReservation` passed to `issue_handover_permit`
  carries no identity — unlike the router's validated `RecipientSlot`. Both are
  real gaps. Both are activation material: the production writer-coverage proof
  and the reservation issuer are what stages 3/4 build, and neither sits on a
  path production takes today. Contrast 4.1, whose counter production already
  reaches.
- **Admission and bounded intents** — stage 2c-ii.
- **F13b-D1**, the dispatched `CommitResources` carrying no present-pin leases
  by value — stage 2c-iii, per the F-13b review.
- **Any production activation** — stages 3/4. Per the stage-2c design's
  section 6, neither 2c-ii nor 2c-iii leaves the production route anything but
  `Legacy` with readiness closed.

## 7. Contract sources

The Owner handover contract is **not** in the 2c-i design used as round 1's
authority, which never mentions `publish_owner`, `HandoverPermit`, `Quiescing`
or `LegacyDrained`. It lives in the 2c-i plan's Task 6 and the stage-2c
design's section 6. By domain:

| Domain | Sources |
| --- | --- |
| Transport gate, Owner handover | 2c-i plan Task 6; stage-2c design §6; R7, R11 |
| GPU batches, availability, completion routing | 2c-i design §4; R9 |
| Commit consumer, KMS release | 2c-i design §4–§5; R6 |
| Reset boundary | 2c-i design §3–§4 (logical vs. retained owners); server-reset design |

## 8. Gate

`cargo +nightly fmt`; `cargo clippy --all-targets -- -D warnings`, in the
default build and with `--features tcp-transport` and `--features xdmcp`;
`cargo test -p yserver --lib c0_2ci` with its hardware run; the twelve-run flake
loop; the full workspace suite; and the gnu, musl and freebsd target checks,
since 4.1 touches `drm_cleanup.rs`.
