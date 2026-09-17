# Phase C.0 stage 2c-i debt — refusal proof, husk identity, reset boundary

**Status:** design, revision 3 (2026-09-16). Codex round 1 on revision 1
(`…-debt-design-review-round1.md`: 3 blocking, 1 major) and round 2 on
revision 2 (`…-debt-design-review-round2.md`: 0 blocking, 3 major) — same
instrument, so the two counts compare — were both verified against the tree
and are incorporated here, together with the author's refusal inventory and
`mod.rs` census (`…-debt-refusal-inventory.md`). Revision 3 also adds a third
part, section 9: a partial run of the flip-accepted path with DRM master, which
this box can only exercise from an active VT such as tty2. Implementation plan
to follow.

**Session 1 executed (2026-09-16): 28 guards proven by oracle; see
`docs/superpowers/findings/2026-09-16-stage-2c-i-debt-census-session-1.md`.**

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
guard, and a family-and-function name is not enough: `apply_teardown_release`
holds two identity guards and `validate_gpu_batch` three (round-2 M-1). So
each guard gets a **stable identifier** — function plus the invariant it
enforces, never a line number — and the association lives **beside each
test**, as a tag in its doc comment naming the identifiers it proves. The tool
reads the tags; no central list exists to drift.

A tagged test failing is still not enough, because it can fail for a reason
unrelated to the mutated guard — an earlier guard, a fixture panic, a shared
postcondition. The tool must also confirm a **guard-specific observable**: the
refusal the guard exists to produce (its error variant, disposition or state)
is the one the test asserted and the one that went missing under the
mutation.

The tool mutates source files and runs cargo. It is a developer tool, not a CI
step, and must restore every file it touches even on failure.

### 3.1. The families

**B. Transport edges** (`transport.rs`, 1). `authorize_write` refuses every
writer class while `Closed` (its `Quiescing` arm is already proven).
`consume_owner_write`'s non-Owner refusal moves to session 2 with family A
(section 4.4): proving it requires a grant issued in Owner, reached today
only through the handover tokens 4.4 reshapes.

**C. Consumer error propagation** (`commit.rs`, 5). `consume`, on
`CompletionRetired`, returns the error when moving the old `Current` into its
reserved retirement slot fails, when moving it into a vacant
`OrdinaryRetirement` fails, and when moving the new `Submitted` into
`Current` fails; `on_available` returns its transition error, in both its
releasing and its rejected halves, rather than `Ok` with admission already
closed. A failure reported as success is the class R9 exists to prevent.

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

That is 28 guards: 27 of the published census's 35 survivors, plus the
`} else if` sibling in family C that the census tool's full enumeration found
and the published census could not see (see
`docs/superpowers/findings/2026-09-16-stage-2c-i-debt-census-baseline.md`). The Owner handover entry's five (family A)
move to session 2, section 4.4, because they must be proven on handover
evidence that session 2 first has to strengthen; proving them now would
certify a state the contract forbids. The two `gpu.rs` survivors are not test
work — see 4.2.

## 4. Session 2 — mechanism changes

This session changes production code, and carries the one test family that
depends on those changes (family A, section 4.4). It is kept apart from session
1 on purpose: if a session-1 test exposes a real defect, there must be no
mechanism change in the same session to fix it into quietly. Family letters are
kept stable across revisions, so session 1 runs from B to I.

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
- the backing entry is still rooted in the service — physical release is gated.

Forced destruction is only half of the reset. The boundary then replaces
session state, and the reset design's invariant 6 is the half that matters
here: "No old-generation object, mapping or queued operation is ever
interpreted as belonging to a newly reused numeric id" — numeric ids are
reused deliberately (round-2 M-2). So the test continues across it:

- the next generation allocates a drawable at **the same numeric XID**;
- the old obligation's proof, arriving late, destroys only the old
  incarnation's entry;
- the new drawable and its backing remain valid and resolvable.

The ledger is likely protected here by construction — `AllocationKey` carries
`device`, `incarnation` and `generation`, so proofs are not keyed by XID — and
the exposed layer is the store's XID-to-drawable mapping across the reset.
"Likely protected by construction" is exactly the untested claim this stage
exists to replace with a test.

Its mutations: make the forced teardown physically destroy a backing whose
obligation is still pending; and make the late proof resolve by XID so it
reaches the new generation. **Not** "drop the lease" — that is required.

The transfer of those retained backings into a teardown supervisor is
**stage 3**, and this test does not claim it. Round 2 confirmed the forced half
is drivable without production wiring: `force_destroy_all_clients` takes the
real `Backend` trait object, `KmsBackend::free_pixmap` reaches the store
decrement path, and a fixture-installed managed allocation is not production
Owner publication under R8. If the generation-replacement half cannot be driven
the same way, that is an F8 stop to report, not a reason to add wiring.

### 4.4. Handover evidence, then the Owner handover entry (round-2 M-3)

`WriterCoverageProof` is an empty token whose test constructor takes no
arguments, and the `RecipientReservation` that `issue_handover_permit` receives
carries no identity and is never inspected. Revision 2 deferred both to stages
3/4 on the grounds that production issuers are activation material. That is
right for the **production** issuers, which stay absent under R8. It is wrong
for the **test** side: the 2c-i plan's Task 6 already requires that tests
construct the coverage proof "only after explicit mock/disabled coverage for
every class", and the stage-2c design allows Owner publication "only when all
writer classes are either owner-mediated or disabled and the teardown receiver
is installed". Tests built on the empty tokens would certify a handover the
contract forbids.

So, first:

- `WriterCoverageProof`'s test constructor consumes explicit evidence — mock
  or disabled — for **every** `WriterClass`, so possessing one proves the
  coverage;
- `RecipientReservation` identifies a compatible recipient slot's device and
  incarnation, and `issue_handover_permit` refuses a reservation for another
  device or incarnation. This is a new production guard, which is why the item
  is in this session.

Then, **family A** on that evidence (`transport.rs`, 5 guards):
`issue_handover_permit` refuses unless the gate is `Quiescing` and no owner
write grant — helper grants included, which is how helper permissions are
modelled — is outstanding; `publish_owner` refuses a permit bound to another
device or incarnation, and refuses unless `Quiescing` with no outstanding
grant. This is the transition into Owner that R7 governs, and none of its
guards was proven.

The `consume_owner_write` non-Owner refusal (family B) is also session-2
scope, after family A's five guards.

### 4.5. Plan corrections (session 2, 2026-09-17)

From prototyping session 2 and from codex round 1 on its plan
(`…-session-2-plan-review-round1.md`). The plan
(`docs/superpowers/plans/2026-09-16-phase-c0-stage-2c-i-debt-session-2.md`)
carries the detail.

1. **4.1.** The husk registration **owns** the alias it accounts for: the
   scanout bo's own `Rc<drm::Device>` moves into it at conversion, and
   consuming the registration drops it. Round 1's B-1 showed that keeping the
   alias on the bo let the inventory reach zero while the alias was still
   alive.
2. **4.2.** The transport gate handle is installed on the `ResourceService`,
   the only thing `submit_shared_scanout_frame` receives, and the handle
   names its gate's device, incarnation and instance so the service refuses a
   foreign gate or a silent replacement (round-1 B-2). A close arriving
   through that handle is terminal: every transition consults the effective
   state, never the raw field (round-2 B-1). The unwind moves into
   `scene::managed_submit_failure`, which both failure arms of the real path
   return as their `Err` value, and which keeps its cause structurally in
   `PresentError::ManagedUnwind`, so a device loss survives a failed unwind
   and still latches the fatal renderer state (round-2 B-2).
3. **4.2 — acceptance, unmet half.** The named tests drive
   `managed_submit_failure`, not the real `scene.rs` path: failing that path's
   unwind needs Vulkan, DRM and fault injection. **4.2's requirement of a
   named test driving the real path is therefore NOT met**, and this
   amendment does not weaken it — it records it as open, next to 4.3's F8
   stop. That a call site still calls the helper is checked by reading, at
   review (round-1 M-1, carried forward as round-2 M-1).
4. **4.2.** The transport closes whenever the submission may have reached
   the GPU, even when the freeze succeeds (2c-i design section 4), and, when
   it provably did not, only if cancelling its obligations fails.
5. **4.3 — F8 stop.** `reset_generation` is `pub(crate)` in `yserver-core`
   and needs a live poller, setup registry and input inventory, so the
   generation-replacement half of 4.3 is **not driven**, and this is the F8
   stop 4.3 itself calls for rather than a reason to add wiring (round-1
   B-3). The test drives the forced teardown through
   `force_destroy_all_clients` and then reuses the numeric XIDs over the same
   backend: it proves proofs are keyed by allocation and not by XID, and it
   does not prove the reset's invariant 6. The crossing stays open for
   whoever owns the boundary next.
6. **4.4.** No public transition reaches `Quiescing` with a grant
   outstanding; the two outstanding-grant tests build that state with
   `set_outstanding_owner_writes_for_tests`. Coverage evidence stays
   test-side and per-class as 4.4 specifies: minting it from fixtures that
   install or disable each writer, and reservations from a live
   `RecipientSlot`, is production-issuer work section 6 defers to stages 3/4
   (round-1 M-2, not taken).
7. **5.1.** The full enumeration is 71 sites (the reservation guard and the
   two `set_transport_gate` guards are new). Session 2 is accepted with
   `CAUGHT_BY_ORACLE` 39, `CAUGHT` 32 and zero survivors, and the three new
   `drm_cleanup.rs` guards proven by oracle with `--files drm_cleanup.rs` --
   with 4.2's real-path half and 4.3's crossing recorded as open.

## 5. Evidence and review

### 5.1. Acceptance

**Session 1:** each of its 28 guards is `CAUGHT_BY_ORACLE` — killed by the test
its tag is bound to, carrying its own marker, under a strategy other than
whole-body replacement. A census over the full enumeration (68 sites: section
2's 67 plus the `} else if` guard) then reports exactly eight survivors, all
session-2 scope:
`issue_handover_permit` (2) and `publish_owner` (3), `consume_owner_write`'s
non-Owner check, and the error arms of `cancel_pre_submit_batch` and
`freeze_uncertain_batch`.

**Part 3:** as stated in section 9 — run by the user on an active VT with DRM
master, mutations included, output reviewed before acceptance.

**Session 2:** 4.1's token fails closed on a foreign, unknown or dropped token,
each proven by a named test; 4.2 and 4.3 as stated in their sections; 4.4's
strengthened evidence proven by construction, its new reservation guard by a
named test, and family A's five guards under the same oracle as session 1.

Both criteria are measured, so they **self-report incompleteness**: an item left
open by an honest F8 stop shows up as a survivor or a failing criterion, and no
one has to rely on the report confessing it.

### 5.2. Review

Per item: the reviewer re-runs the named mutation, addressed by line number at
review time and confirmed to have compiled. Per family, as families land. The
global census re-run closes session 1.

### 5.3. Risk

Families C, E and F in session 1, and family A in session 2, were whole
unproven paths, and unproven paths are where real defects hide. If a session-1 test cannot pass because the code is wrong,
that is an F8 stop: report it, leave the family open, and decide the fix
separately with its own review. The session split exists so that this is a
boundary, not a temptation.

## 6. Out of scope

- **Production issuers for the handover evidence** — stages 3/4. The
  production writer-coverage proof and the production reservation issuer are
  what stages 3/4 build, and R8 keeps both absent. Their **test-side**
  strengthening is not deferred: it is section 4.4, after round 2 showed that
  tests built on the empty tokens would certify a forbidden handover.
- **Admission and bounded intents** — stage 2c-ii.
- **F13b-D1**, the dispatched `CommitResources` carrying no present-pin leases
  by value — stage 2c-iii, per the F-13b review.
- **The bounded delivery check** of the C.0 design's section 16.3 revision 3 —
  stages 3/4. It exercises real transitions through owner machinery that is
  complete only then. Section 9 is not it and must never be reported as it.
  See `docs/superpowers/findings/2026-09-16-phase-c0-verification-regime-decision.md`.
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

## 9. Part 3 — the flip-accepted path, with DRM master

### 9.1. Why this box has never run it

The hardware test fixtures open the card deliberately **without DRM master**
(`backend.rs`, around line 6003), because the desktop session already holds it.
Without master the kernel rejects page flips, so the flip-accepted path has
never run on hardware here: a managed buffer actually going on screen, a real
page-flip completion, a real out-fence. Every test that exercises the discharge
of a `KmsRelease` obligation feeds it a synthesized completion event. The census
cannot see this gap — it asks whether a test notices a deleted guard, not
whether the evidence that test uses is real.

### 9.2. What it must show

Stated as invariants; the shape of the tests is the implementer's call.

- **P3-1.** A managed scanout buffer submitted in a flip the kernel **accepts**
  becomes the current buffer — the flip-accepted path actually executes.
- **P3-2.** The displaced buffer's `KmsRelease` obligation is discharged by the
  **real** kernel completion, and the buffer is not released before it.
- **P3-3.** A buffer retained across the flip registers no obligation and is not
  released — R6's retained-member clause, with real evidence.
- **P3-4.** The flip's out-fence resolves through its canonical status query.

Every test here carries the `_drm` suffix and `#[ignore]`, and **detects whether
it holds master**. Run without it, the test reports an environmental skip as a
failure (R12) — it never passes. The implementer also identifies which existing
hardware tests encode the no-master outcome, by running them with master, and
records the result rather than assuming it.

### 9.3. How it is run

The implementer writes the tests and cannot run them: DRM master is granted by
logind to the **active** session on the seat, and an agent's shell runs inside
the session that launched it. So the user runs them:

1. switch to a VT that becomes the active session on the seat — on this machine
   tty2 — so the desktop on tty1 drops master;
2. run the exact commands the plan supplies, which select the card driving the
   connected output (this machine has two) and report which card was used;
3. run the named mutations for P3-2 and P3-3 with the census tool from section
   3.0, under the same filter;
4. save both outputs to a file the reviewer reads.

### 9.4. What it is not

It is **not** the bounded delivery check of the C.0 design's section 16.3
revision 3. That check exercises DPMS, VT release and acquire, and direct,
composed and fullscreen entry and exit through the owner machinery, which is
complete only in stages 3/4, and it is scheduled there. Part 3 is an early,
partial falsification of the stage 2c-i ledger on real kernel evidence. It
satisfies no section-16.3 requirement and is never reported as doing so.
