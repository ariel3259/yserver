# Phase C.0 stage 2c-iii — the copied scanout route

**Status:** design, revision 2 (2026-09-22). Its sections were approved one by
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

**CP-3 — the owner buffer state machine gains no state.** `Rendering`
(`owner_buffer.rs:29`) already means "the producer chain has not finished
writing this buffer", which is true during both stages. What discriminates them
is the producer stage the generation is waiting on, beside the existing
`OwnerRenderWaiting` (`scene.rs:111`); how that distinction is represented is
the plan's choice. `into_desired` happens at B's retirement,
not at A's. `Displaced` applies identically in both stages, with no new rule.

### 3.2. Ownership while the copy is in flight

**CP-4 — batch B owns both halves for the duration of the copy.** The batch
registered for the sink copy carries a write `GpuObligation`
(`resources/gpu.rs:30`) over the **destination** allocation — that obligation's
retirement is what makes it readable — and a `ReadObligation`
(`resources/gpu.rs:91`) over the **source** allocation. A's own batch retires
A's own leases and is never the authority that frees the source.

The batch is device-agnostic by construction: `GpuObligation` carries its own
`Arc<VkContext>` (`resources/gpu.rs:33`), so a batch produced on the sink device
is as valid to the service as one produced on the renderer device.

**CP-4a — the A-to-B transition moves leases; it never re-acquires them
(round-1 B-1).** How ownership crosses the boundary is not left to the
implementation, because the obvious orders are both wrong. The copied
acquisition already holds **two `Write` leases** for the generation: the
destination (`platform.rs:6347`) and the renderer source (`platform.rs:6355`).
Re-reserving the source as `Read` for B while A's lease is live fails `Busy` —
`is_compatible` refuses a read against a live writer
(`resources/availability.rs:116`) — and dropping A's lease first opens an
interval in which the source has no live use at all and the pool may hand the
slot out. Therefore:

1. The leases the producer already holds are **transferred by value** into B's
   batch. `CoreRetirementBatch.leases` and `ReadObligation::new` both take
   leases by value, so this is a move, not a new acquisition, and
   `is_compatible` is never consulted on this path.
2. The obligations are minted with `ResourceService::register`
   (`resources/mod.rs:865`), which records a pending obligation on the entry and
   does **not** consult `is_compatible`. Obligations and leases therefore move
   independently, which is what makes this transition expressible at all.
3. Submitting the copy, minting both obligations, moving both leases into the
   batch and registering the batch are **one synchronous step with no event-loop
   yield inside it**. There is no point at which the source is reachable by
   another acquirer.
4. Obligations are minted only after the copy submission has succeeded. Once it
   has, the batch is registered with whatever it holds, so GPU work never leaves
   leases orphaned outside the service.

**CP-4b — this route is `ReadObligation`'s first production caller.**
`ReadObligation::new` and `bind_read_obligation` are today driven only by
`resources/guard_tests.rs:202`, `:221`, `:244` and `resources/tests.rs:1851`.
Resting CP-5 on machinery that only tests call is the defect class plan Ciii's
hardware run found as F-T6-4, so it is stated here rather than discovered later:
the plan's evidence must show the production path registering the read
obligation, and a test that calls it by hand does not satisfy any criterion of
section 8.2.

**CP-5 — the source outlives the copy.** While B has not retired, the source
allocation cannot return to the renderer pool. A copied generation whose source
was recycled before its copy retired is a defect of this route, not of the
service.

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
| The transition moves the producer's leases and re-acquires nothing (CP-4a) | Re-reserve the source as `Read` for the batch; release the producer's source lease before the batch takes it |
| The read obligation is registered by the production path, not by a test (CP-4b) | Drive the criterion from a hand-built batch instead of the route |
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
| The copied route works cross-device on real hardware (section 6) | The exclusivity mutation of CP-8, run on hardware under the same filter |

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
