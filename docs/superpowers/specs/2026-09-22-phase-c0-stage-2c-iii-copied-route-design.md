# Phase C.0 stage 2c-iii — the copied scanout route

**Status:** design, revision 4 (2026-09-22). Its sections were approved one by
one with the user in brainstorming, together with the three decisions recorded
in section 2: the evidence level, the placement of the copy's completion wait,
and the managed-storage access debt staying out of scope.
Revision 2 incorporates codex round 1
(`../findings/2026-09-22-stage-2c-iii-copied-route-design-review-round1.md`,
instrument `da807b70` with 24 excerpts: 1 blocking, 2 major, all three verified
against the tree and accepted): the A-to-B ownership transition is named rather
than assumed (B-1, section 3.2), a successful copy whose wake registration fails
has a disposition (M-1, section 3.3), and the destination's write obligation has
its own mutation and its own evidence (M-2, sections 6 and 8.2). While verifying
B-1 the author found that `ReadObligation` had no production caller; section 3.2
now says so and makes this route its first.
Revision 3 incorporates codex round 2
(`../findings/2026-09-22-stage-2c-iii-copied-route-design-review-round2.md`,
same instrument, 24 excerpts: 1 blocking, 0 major; M-1 and M-2 audited as
applied). Round 2 carried B-1 forward as partial, and verifying it showed that
revision 2's own premise was false: the acquisition leases protect selection
only and are dropped there (`scene.rs:6194`-`6201`), so there was never a
producer lease to move. Section 3.2 is rewritten from the sequence the code
runs, and the copied route now follows `prepare_retirement_batch`'s established
idiom — every obligation registered before any raw handle reaches the GPU, the
whole attempt unwound on failure — instead of a transaction invented for it.
Revision 4 incorporates codex round 3 (`../findings/2026-09-22-stage-2c-iii-copied-route-design-review-round3.md`,
same instrument, 24 excerpts: 1 blocking, 3 major, 1 minor; the prior B-1 is
audited as traded, M-1 and M-2 of round 1 as applied). All five verified: the
offer had no correlated retirement receipt (B-1, new CP-2a); an uncertain
dispatch cannot be carried by a ticketless batch, because quarantine takes its
keys from the obligation the batch does not have (M-1, CP-4b rewritten onto
`abandon_unsubmitted_batch`); the source's exclusion is the paired BO phase, not
non-interleaving, so revision 3's mutation could not have failed (M-2, CP-4c
rewritten); the cross-device criterion was paired with an exclusivity mutation
that does not test it (M-3); and the status line still said revision 2 (m-1).

This is the plan that plan Ciii's acceptance finding
(`../findings/2026-09-22-stage-2c-iii-plan-ciii-accepted.md`, "Carried") records
as still owed to stage 2c-iii. It exists as its own design because the
correction that removed the copied route from plan Ciii — round 1 B-2
(`../findings/2026-09-20-stage-2c-iii-plan-ciii-review-round1.md`) — left a real
architectural question open, not merely a task: the copied route's flip consumes
a GPU fence as `IN_FENCE_FD`, and the owner's host call cannot carry one.

**Authority**, most general first. This document elaborates the 2c-iii block; it
does not replace or relax any of it.

1. [C.0 specification](2026-08-26-phase-c0-atomic-kms-migration-design.md): §9
   (the owner and admission), §10 (the accepted-commit lifecycle, §10.2
   retirement milestones, §10.4 Present and release terminalization), §12.1 (the
   `DMG` damage transaction) and §13 (multi-device).
2. [Stage 2c-iii design](2026-09-19-phase-c0-stage-2c-iii-conversion-design.md):
   §3 (architecture and the prepare/submit selection boundary), §4 (plan Ci — the
   composed producer, whose shape this route reuses exactly), §6.3 (route
   selection and exclusivity, including R8) and §8 (verification). The copied
   route is listed there as Ciii's carried item; this document is where it lands.
3. [Stage 2c-i design](2026-09-08-phase-c0-stage-2c-i-resource-terminalization-design.md)
   and its [debt design](2026-09-15-phase-c0-stage-2c-i-debt-design.md): the
   resource ledger, the managed adoption of both pool halves, and the
   read/write obligation model this route depends on.
4. [Stage 2c-ii design](2026-09-18-phase-c0-stage-2c-ii-admission-design.md): the
   decider and the conductor. This route feeds them one more real producer; it
   changes no admission rule.

Where this document names *how*, it is new; where it names *what*, it cites the
section above that already requires it.

## 1. What this delivers, and what it does not

**Delivers:** the copied composed producer (`submit_copied_scanout`,
`platform.rs:6395`) converted to the Owner route, the first fixture in the
project that builds a copied output, the exclusivity case and mutation that
Ci could only cover in the pure validator (R8), and the eligibility change that
lets a device with a copied-route output enter `Owner` at all.

**Does not:**

- activate the owner route in production — stages 3/4 still own that boundary
  (2c-iii §7);
- change any admission, completion or retirement rule: every rule this route
  obeys is Ci's, reached by a producer with one more GPU stage;
- convert the unmanaged-pool refusal into eligibility — a pool half without a
  managed adoption stays refused (section 4);
- touch cursor, gamma, lifecycle or topology dispatch (stages 3 and 4);
- repair the managed-storage access debt (section 7).

## 2. Decisions taken in brainstorming

**2.1. Evidence level: fixture plus one hardware test** (user, 2026-09-22). Same
shape as 2c-iii §6.4: everything is proven at fixture level with the real
producers, conductor and owner, and one real-device test establishes that the
cross-device route works on hardware. This machine has two GPUs — `card0`
amdgpu with every connector disconnected, `card1` nvidia with `HDMI-A-2`
connected — so a real `RenderKmsRelationship::Different` route is reachable by
rendering on the iGPU and scanning out on `card1` (section 6).

**2.2. The copy's completion is proven by the resource service, woken by its
fence** (user, 2026-09-22; approach A of three). Round 1 B-2 named two
permissible corrections: a nonblocking wake after the fence signals, or extended
owner IPC that transfers and patches input fences. The IPC extension was
rejected: host→helper requests are byte-only frames (`executor/mod.rs:851`,
`transport.rs:63`), `SCM_RIGHTS` exists only helper→host for out-fences
(`transport.rs:67`), and carrying a GPU dependency into the executor would put
fence lifetime under `ExecutorStalled`, supersession and reap — all of it new
substrate semantics on an already accepted stage-1/2a boundary. A pure epoll
stage was also rejected: it would make a file descriptor the authority for
readability, beside the resource service, which is the authority everywhere
else. What was chosen keeps both roles but does not merge them, exactly as the
composed route already does.

**2.3. The managed-storage access debt stays out of scope** (user, 2026-09-22).
Named, not repaired: section 7.

## 3. Architecture

### 3.1. The copy is the producer's last stage

The copied route has two GPU stages on two devices: renderer A writes the
**source**, sink B copies into the **destination**, and the destination is what
KMS scans out. Today A's completion file descriptor is handed straight to
`submit_copy` (`vk/scanout.rs:1809`), whose returned fence becomes the legacy
flip's `IN_FENCE_FD` (`platform.rs:6461`-`6470`): the kernel performs the wait.

The conversion does not move the route fork to a different function. It makes
**B the last stage of the producer**, so that at the offer boundary the
conductor sees exactly what it sees for a shared pool:

```text
A completes (its fd, as today)
  → submit the sink copy with a fence ticket from the sink context
  → register that copy as a CoreRetirementBatch with the resource service
  → [ the copy's sync_file wakes the core loop through the CompletionPoller
      that is already registered there (platform.rs:4497) ]
  → service_completions retires the write obligation on the destination
  → the generation becomes Desired, and only then is a ComposedOffer pushed
  → admission, owner commit, no IN_FENCE and no descriptor in the request
```

**CP-1 — the offer follows readability, never submission.** A copied generation
may be offered to the conductor only after the resource service has retired the
write obligation covering its destination allocation. This is Ci's rule, not a
new one: `scene.rs:3959` already records why the composed route services the
completion before offering — offering first leaves readiness stuck on `Busy` in
the same wake.

**CP-2 — the fence wakes, the service authorizes.** The copy's `sync_file`
exists to make the core loop run again; it is never the proof that the
destination is readable. The two roles are separate and must not be merged into
one mechanism.

**CP-2a — the promotion consumes a correlated retirement receipt (round-3
B-1).** "The service authorizes" is not a wake and not a call: it is one
explicit answer about **this** generation's obligation. At CP-4a step 2 the
generation keeps the receipt `(destination AllocationKey, ObligationId)`, and it
may become `Desired` and be offered only when both hold:

- `has_pending_obligation(destination_key, obligation_id)` is false
  (`resources/mod.rs:290`) — that exact obligation retired, not merely some
  obligation on some key;
- `is_frozen(destination_key)` is false (`resources/mod.rs:911`) — it was not
  quarantined on the way.

Everything else is explicitly rejected as authority: `service_completions`
returns generic keys and a service-wide error (`resources/mod.rs:359`), so
neither its `Ok` nor its `Err` decides this generation, and the composed
precedent — which logs a service failure and offers anyway (`scene.rs:3965`) —
is **not** the shape to copy here. A readable fence with a pending or frozen
obligation offers nothing.

**CP-3 — the owner buffer state machine gains no state.** `Rendering`
(`owner_buffer.rs:29`) already means "the producer chain has not finished
writing this buffer", which is true during both stages. What discriminates them
is the producer stage the generation is waiting on, beside the existing
`OwnerRenderWaiting` (`scene.rs:111`); how that distinction is represented is
the plan's choice. `into_desired` happens at B's retirement,
not at A's. `Displaced` applies identically in both stages, with no new rule.

### 3.2. Ownership across the two stages

**The sequence the code already runs, which this route extends.** Revision 2
described a handoff that does not exist, so revision 3 starts from the tree:

- **Selection.** `acquire_managed_scanout_bo` takes a lease on each half
  (`platform.rs:6347` destination, `platform.rs:6355` source) and the caller
  **drops them immediately**. They protect selection only, and the reason is
  written at the site: holding one across the compose's own reservation "would
  make the service correctly report `Busy` for the same allocation"
  (`scene.rs:6194`-`6201`).
- **Compose.** `prepare_retirement_batch` (`resources/gpu.rs:299`) reserves the
  write lease and registers the GPU obligation for every allocation the
  submission will touch **before the caller may hand any raw handle to the
  GPU**, and a failure partway through releases everything already reserved for
  that attempt, so no half-registered batch can survive.
- **Completion.** The generation takes its `Retain` lease, the batch is
  registered and serviced, and its write lease drops with it
  (`resources/mod.rs:1560`).

On the copied route A writes the **source**; the destination is written by B.

**CP-4 — one holder at a time, and no lease crosses a batch.** A's batch owns
the source write lease and the source GPU obligation and retires both when it is
serviced. B's batch owns the destination write lease with its GPU obligation and
the source read lease with its `ReadObligation` (`resources/gpu.rs:91`). No lease
is shared between the two batches, taken out of a registered batch, or acquired
while another holder is live.

The batch is device-agnostic by construction: `GpuObligation` carries its own
`Arc<VkContext>` (`resources/gpu.rs:33`), so a batch produced on the sink device
is as valid to the service as one produced on the renderer device.

**CP-4a — B is prepared exactly as A is: obligations before GPU work, unwound
as a whole (round-2 B-1).** At A's completion, in this order:

1. A's batch is registered and serviced; its source write lease drops with it.
2. B reserves the destination `Write` and registers its GPU obligation. The
   destination has no live holder — its selection lease was dropped at
   selection and nothing else took one.
3. B reserves the source `Read` and registers its read obligation. This is now
   compatible: `Read` requires no live writer (`resources/availability.rs:116`)
   and A's writer is gone.
4. **Only then** is the copy submitted.
5. The batch is built owning both leases and both obligations, its ticket is
   bound, and it is registered with the service.

A failure at step 2 or 3 cancels every obligation already registered for the
attempt and drops every lease already taken, the way `cancel_pre_submit_batch`
(`resources/gpu.rs:370`) does for a submission that provably never reached the
GPU. Nothing is submitted and the generation is `Displaced`.

**CP-4b — a failed submission takes the established unwind, by its
`gpu_submitted` answer (round-3 M-1).** Revision 3 said an uncertain dispatch
registers a ticketless batch with `possibly_dispatched` set. That loses the
destination: `quarantine_gpu_batch` collects the keys it freezes from the
batch's `Option<GpuObligation>` and its read obligation
(`resources/mod.rs:1563`), so a batch with no ticket freezes no destination key
at all. The route therefore uses the path the project already has —
`abandon_unsubmitted_batch` (`resources/gpu.rs:395`), which implements the 2c-i
debt rule "unknown submission retains its reservation and closes the affected
transport":

- **provably not dispatched:** cancel every prepared obligation, and close the
  transport gate only if that cancel fails;
- **may have dispatched:** close the transport gate and freeze the prepared
  entries, both the destination and the source.

Either way the generation is `Displaced` and the prepared entries are disposed
of by key, never by the presence of a ticket.

**CP-4c — the source is excluded by the paired BO phase, not by
non-interleaving (round-3 M-2).** Between A's write lease dropping and B's read
lease being taken the source has no live lease, and revision 3 claimed the core's
lack of a yield is what keeps a later tick from taking it. That is not the
mechanism the tree runs. Selection scans the **destination** pool for bos in
`BoPhase::Free` (`platform.rs:6337`) and transitions the chosen one to
`Recording` before returning the token (`platform.rs:6374`), precisely so that
"the slot stays `Free` and legacy `acquire_scanout_bo` can hand out the same
index" cannot happen. The source is selected by the same `bo_idx`, so while the
destination is `Recording` — which it is from selection until the copy is
submitted — no later tick can select that source at all. The authoritative
exclusion is therefore the paired phase, and that is what section 8.2 mutates:
skip or release the destination's `Recording` transition. No claim about
event-loop scheduling is made or needed, and none of the fallible steps above
rests on one.

**CP-4d — this route is `ReadObligation`'s first production caller.**
`ReadObligation::new` and `bind_read_obligation` are today driven only by
`resources/guard_tests.rs:202`, `:221`, `:244` and `resources/tests.rs:1851`.
Resting CP-5 on machinery that only tests call is the defect class plan Ciii's
hardware run found as F-T6-4, so it is stated here rather than discovered later:
the plan's evidence must show the production path registering the read
obligation, and a test that calls it by hand satisfies no criterion of
section 8.2.

**CP-5 — the source outlives the copy.** From step 3 until B retires, the source
allocation cannot be reserved for writing or returned to the renderer pool. A
copied generation whose source was recycled before its copy retired is a defect
of this route, not of the service.

### 3.3. Failure, cancellation and supersession

- **The copy fails to submit.** The existing quiesce path is kept
  (`recover_copy_failure`, `vk/scanout.rs:2530`, and the `renderer_failed`
  fail-stop when quiescing itself fails). On the Owner route the generation
  becomes `Displaced` — nothing is offered — and its leases are discharged by
  the service, never by hand.
- **Output removal, device loss, VT release.** The cancellation scope for the
  copy wait is the same as A's
  (`cancel_scanout_render_completions_for_output`, `platform.rs:4968`). The
  pending copy is value-owned, so cancelling it is dropping it; a batch already
  registered continues its normal course in the service to retirement or
  quarantine.
- **The copy submits but its completion wake cannot be registered (round-1
  M-1).** Registering a completion is fallible and its existing caller
  propagates the failure (`scene.rs:6577`). B has already submitted here, so the
  batch stays registered — its leases and obligations stand and the service's
  own deadline path retires or quarantines it — the generation becomes
  `Displaced`, any partial waiter state is removed, and **no offer is made**.
  Generic availability is never an offer signal: only the correlated retirement
  of this generation's destination obligation is.
- **A newer generation supersedes.** Identical to composed: the in-flight copy is
  not aborted, its offer is discarded, and the buffer is `Displaced`.
- **Deadline expiry and `renderer_failed`.** The batch's own serviced deadline
  (`resources/gpu.rs:164`) quarantines that batch alone, under rules already
  accepted. This route adds no new expiry path.

**CP-6 — no owner commit on this route carries an input fence, and no
host→helper request carries a descriptor.** The wait is discharged before the
commit is built, so the request is byte-only like every other.

## 4. Eligibility and exclusivity

**CP-7 — eligibility requires both halves adopted.** `check_owner_eligibility`
(`platform.rs:3425`) today classifies every non-`Shared` pool as
`OwnerOutputKind::Copied` and refuses. A copied pool becomes eligible when its
`destinations` **and** its `sources` are managed; `display_pool()` already
resolves to `destinations` for a copied pool (`vk/scanout.rs:774`), and the
source half needs its own check. If either half has a bo without a managed key,
the refusal survives as `Unmanaged`. No refusal is dropped: one changes its
reason, and `NoOutputs`/`Missing` are untouched.

**CP-8 — no legacy primary write on an Owner device, proven at this site.**
`submit_copied_scanout` (`platform.rs:6395`) is the last primary submit site
without an Owner case. Per 2c-iii §6.3 it carries its own mutation — force the
legacy branch there — which must break a named test, and **the transport gate's
refusal is not accepted as that test's observation**, because the gate is the
defence the fork is supposed not to need.

This is also what closes R8. Plan Ci's acceptance
(`../findings/2026-09-19-stage-2c-iii-plan-ci-accepted.md`) could cover the
copied route only in the pure validator because no fixture built a copied
output; the fixture this design requires is the first one, and it is what makes
the exclusivity case executable rather than validator-only. Round 1 M-1 — that
copied-route evidence must not be demanded before the route is reachable — is
satisfied structurally here: reachability and evidence are in the same document.

## 5. Retirement and release

**CP-9 — the destination retires under Ci's rules, unchanged.** The owner ledger
is the authority; `KmsRelease` is discharged by the real completion; the pool
slot is released only when every gate of 2c-iii §4.2 has passed, never at the
ack.

**CP-10 — the source is released by B's read obligation.** Not by the ack, not
by A's batch, and not by the flip's retirement.

**CP-11 — retention registers nothing.** Two generations that share the same
destination allocation for the same member register no new obligation and
release nothing — the P3-3 invariant that plan Ciii proved on hardware, restated
for this route because its destination is filled by a second stage.

## 6. The hardware test

From tty2 on a machine with both GPUs, asking the user first (the GPU is in
personal use). The renderer is forced to the amdgpu iGPU (`card0`) while KMS and
the connected output stay on `card1`/`HDMI-A-2`, which is a real
`RenderKmsRelationship::Different` route rather than a fixture's construction.

With the real conductor, helper and producers, one run drives: a composed frame
on the copied route — render, copy, offer, `Accepted`, `HardwareComplete`, damage
applied — and proves that the destination was not offered before its obligation
retired. Per round-1 M-2 that ordering claim is circular unless the evidence
**first records that the destination's write obligation was registered under the
destination's own key, and then records its retirement**. Fence order, or the
mere fact that the service was called, does not establish it. The exclusivity mutation of CP-8 runs on the same hardware under the
same filter, as 2c-iii §6.4 requires of its own mutations.

**CP-8's mutation does not test this criterion (round-3 M-3).** Forcing the
legacy branch proves Owner/Legacy exclusivity, which is already its own
criterion, and it can fail for route-gating reasons without the sink copy ever
having been exercised. The hardware run therefore records the **distinct
renderer and sink device identities** it actually used, and carries its own
mutation: misroute the copy so it is not performed on the sink's device — or
scan out the source instead of the destination — which must fail this criterion
while leaving CP-8's untouched.

If the forced-renderer configuration turns out not to be reachable on this
machine, that is reported as an F8 stop with the evidence, not substituted by a
fixture and not silently downgraded.

## 7. Out of scope, and the doors that stay open

- **The managed-storage access debt** (user, 2026-09-22; carried from Cfb §2.5).
  Named site: the scanout readback resolves `managed_key` as `None` for a
  `Copied` pool (`backend.rs:18716`), so it reads through an unmanaged path an
  allocation that is in fact managed. With the destination becoming an owner
  authority, such a read can cross the owner's writes. This design does not
  repair it and does not depend on it; it is declared here so a review finds it
  declared rather than discovers it.
- **Production activation** — stages 3/4 (2c-iii §7).
- **Direct scanout and unflip on a copied-route device.** Those producers are
  already converted (Cii, Cfb, Ciii) and select on the device, not on the pool
  route; this design changes neither. What it does require is that an unflip
  returning to composed on a copied output returns to the two-stage chain: the
  retained framebuffer for such an output is its destination bo, and DMG-5's
  invalidate-and-repaint runs through both stages before it is scanned out again.
- **Phase C.1** — untouched.

## 8. Verification

### 8.1. Level

Fixture level with the real producers, the real conductor and the real owner,
plus the single hardware test of section 6. Every finding and status line states
which. The fixture that builds a copied output is part of the deliverable, not a
precondition borrowed from elsewhere.

### 8.2. Exit criteria

Each criterion gets a named test and a named mutation in the plan; mutations are
confirmed to have compiled and to remove the behaviour, and are applied by line,
not by the first textual match.

| Criterion (source) | Mutation that must fail it |
| --- | --- |
| A copied generation is offered only after its destination's write obligation retired (CP-1) | Offer at copy submission; offer at A's completion |
| The fence wakes and the service authorizes; neither substitutes for the other (CP-2) | Treat the copy's sync_file readability as proof of readability and skip the service |
| The owner buffer reaches `Desired` at B's retirement, not A's (CP-3) | Promote to `Desired` when A completes |
| A displaced generation behaves identically in either producer stage (CP-3) | Offer a generation displaced during the copy |
| Batch B holds a write obligation on the destination and a read obligation on the source (CP-4) | Register the batch without the read obligation; **and, separately, register it with no destination obligation, or with one keyed to another allocation** (round-1 M-2) |
| Every obligation B needs is registered before the copy reaches the GPU (CP-4a) | Register the destination obligation after submission; register the source obligation after submission |
| A failed preparation leaves no obligation and no lease behind (CP-4a) | Fail step 3 and keep the destination obligation; fail step 3 and keep its lease |
| A failed submission is disposed of by its `gpu_submitted` answer, by key (CP-4b) | Cancel the prepared obligations on an uncertain dispatch; dispose of an uncertain dispatch through a ticketless batch, so no destination key is frozen; leave the transport gate open on an uncertain dispatch |
| The source is excluded by the destination's paired `Recording` phase (CP-4c) | Skip the destination's `Recording` transition at selection; release it before the copy is submitted |
| The promotion consumes this generation's own retirement receipt (CP-2a) | Promote on a readable fence while the obligation is still pending; promote while the destination key is frozen; promote on a successful `service_completions` that retired another key |
| The read obligation is registered by the production path, not by a test (CP-4d) | Drive the criterion from a hand-built batch instead of the route |
| A copy that submitted but could not register its completion wake offers nothing and keeps its batch (3.3) | Offer on generic availability after a failed wake registration; drop the batch |
| The source is not reusable until B retires (CP-5) | Return the source to the renderer pool at A's completion |
| A failed copy submission offers nothing and discharges its leases through the service (3.3) | Offer after a failed copy; release its leases by hand |
| Cancellation covers the copy wait with the same scope as A's (3.3) | Cancel only A's pending completions on output removal |
| No owner commit on this route carries an input fence, and no request carries a descriptor (CP-6) | Attach the copy fence to the commit request |
| A copied output is eligible only when both pool halves are managed (CP-7) | Accept a copied output whose sources are unadopted |
| No legacy primary write is issued at the copied submit site on an Owner device (CP-8) | Force the legacy branch at that site — and the transport gate's refusal does not count as the observation |
| The destination retires under the ledger and every §4.2 gate (CP-9) | Release the pool slot at the ack; drop one gate |
| The source is released by B's read obligation alone (CP-10) | Release the source at flip retirement |
| A retained destination allocation registers no obligation and is not released (CP-11) | Register the retained allocation as if displaced |
| The copied route works cross-device on real hardware (section 6) | Misroute the copy off the sink's device, or scan out the source instead of the destination — CP-8's mutation does not test this (round-3 M-3) |

### 8.3. The plan and its process

One implementation plan, targeted at 8-10 tasks; if it exceeds roughly twelve
while being written it is split into two before review, per the measured
relationship between plan size and defect density
(`../findings/2026-09-03-phase-c0-stage-2-plan-review-round2.md`). The plan is
reviewed through `../review/review.sh` and its findings are written to
`../findings`, as AGENTS.md requires. Per-task gates, not per-plan: each task
carries the full gate including clippy.

### 8.4. Gate

Per task: `cargo +nightly fmt`, `cargo clippy --all-targets -- -D warnings`, the
crate's tests, and the stage filters. Before acceptance: `cargo check
--workspace` for Linux glibc, Linux musl and FreeBSD; the library suite run
repeatedly where helper-spawning suites are involved; then the hardware run of
section 6 with its mutation.

## 9. Questions for the plan

1. Where the sink's fence tickets come from. `submit_copy_with_fence`
   (`vk/scanout.rs:1817`) already accepts a `vk::Fence`, but only the probe uses
   it; the plan decides how a `FenceTicket` is sourced for the sink context and
   whether a pool is needed there.
2. Whether the copy wait registers in the existing `CompletionPoller` or gets its
   own, given that its entries are keyed by a render job id today
   (`platform.rs:4891`). Either is acceptable; the plan states which and why, and
   CP-2 holds regardless.
3. How the fixture builds a copied output without a second GPU, and what it is
   allowed to stand in for. It may not stand in for section 6.
4. Whether any site other than the readback of section 7 resolves a copied
   pool's managed key as absent. If one is found, it is reported, not repaired.
5. Which production consumers other than tick selection can reserve a copied
   pool's source allocation. CP-4c rests on the paired BO phase excluding a
   later tick; the plan enumerates the rest and states, for each, why it cannot
   take the source between A's retirement and B's read reservation. An
   unenumerated consumer is an F8 stop, not an assumption.
