# Phase C.0 stage 2c-iii — primary conversion and damage

**Status:** design, revision 1 (2026-09-19). Its three design sections (plans
C1, C2 and C3 — here sections 4, 5 and 6) were approved one by one with the user
in brainstorming, together with three decisions recorded in section 2: the
evidence level, the split into three plans with the composed plan first, and
the prepare/submit selection boundary. Not yet reviewed; implementation plans
follow the codex review of this document (section 8.3).

**Authority**, most general first. This document elaborates the 2c-iii block; it
does not replace or relax any of them.

1. [C.0 specification](2026-08-26-phase-c0-atomic-kms-migration-design.md): §9
   (the owner and admission), §10 (the accepted-commit lifecycle, §10.2
   retirement milestones, §10.4 Present and release terminalization), §12
   (interaction with Phase B direct scanout), §12.1 (the `DMG` damage
   transaction) and §13 (multi-device).
2. [Stage 2c design](2026-09-08-phase-c0-stage-2c-conversions-and-damage-design.md):
   §1 (the integration sites), §2 (2c-iii's inputs, deliverable and exit
   evidence), §5 (the damage contract and the current-master scene contracts),
   the v1.5.0 table, §6 (activation and the transport state) and §7 (the open
   design questions).
3. [Stage 2c-i design](2026-09-08-phase-c0-stage-2c-i-resource-terminalization-design.md)
   and its [debt design](2026-09-15-phase-c0-stage-2c-i-debt-design.md): the
   resource ledger, the six direct-resource roles, the handover evidence of the
   debt spec §4.4 and the F8 stop of its §9.5.
4. [Stage 2c-ii design](2026-09-18-phase-c0-stage-2c-ii-admission-design.md):
   the decider, the conductor, the maintenance store and the receipt. 2c-iii
   feeds them real producers; it does not change an admission rule.

Where this document names *how*, it is new; where it names *what*, it cites the
section above that already requires it. A C.0 statement that does not hold in
code is recorded here as an explicit correction or F8 stop, never as a silent
divergence.

## 1. What 2c-iii delivers, and what it does not

From the stage 2c design, §2: **converted composed, direct and unflip paths and
commit-bound damage transactions behind an exclusive transport boundary.** Exit
evidence: end-to-end producer/owner tests, the damage milestone matrix, grouped
retirement and legacy exclusion checks — section 8.

2c-iii does **not**:

- activate the owner route in production (R8; stage 2c §6): until stages 3/4
  convert or disable the remaining writer classes, every production device stays
  `Legacy`, and a `Legacy` device behaves exactly as today;
- choose its own admission order (stage 2c §2) — every converted producer enters
  2c-ii's conductor;
- release resources through the old page-event shortcut (stage 2c §2);
- implement cursor or gamma producers, the cursor coordinate lane, lifecycle,
  topology or recovery dispatch (stages 3 and 4), or anything of Phase C.1;
- revise C.0 §16.3 — revision 5 is owed before stages 3/4, not here.

**Carried items** from 2c-ii (plan B2 acceptance finding, "Limits carried to
2c-iii") and their home:

| Item | Home |
| --- | --- |
| Real `CommitDescription` builders and producer readiness | C1 (composed), C2 (direct), C3 (unflip) |
| The real direct-eligibility predicate, extracted from `try_present_direct` | C2 |
| Real layout-change hook sites | C2 |
| F13b-D1: dispatched `CommitResources` carries the present-pin leases by value | C2 |
| Ready-unflip dispatch (needs a retained composed framebuffer) | C3 |
| Multi-device conductor state | C3 |

## 2. Decisions taken in brainstorming

**2.1. Evidence level (user, 2026-09-19).** Fixture level, as in 2c-ii, **plus
one hardware test** from tty2 that puts card1 in `Owner` and drives a composed
frame, a direct frame and an unflip through the real conductor, the real helper
and the real producers (section 6.4). It is the first evidence that the owner
route works on a real device; the precedent is part 3 of the 2c-i debt stage.
Activating production was rejected: it contradicts stage 2c §6 and R8.

**2.2. Three plans, composed first (user, 2026-09-19).** C1 — composed producer,
damage transaction, bundles. C2 — direct producer, eligibility, layout hooks,
F13b-D1, retirement promotion. C3 — unflip, multi-device, route selection,
hardware test. Composed goes first because it is the base traffic every other
path returns to, and damage is the contract with the most risk. Keeping damage
in C1, rather than a plan of its own, avoids an intermediate state in which the
owner route stages damage at submission.

**2.3. The selection boundary: prepare / submit (user, 2026-09-19).** This
answers the third question stage 2c §7 left open ("where is the exclusive
production-route selection"). Each producer splits into:

- **prepare** — shared by both routes: composing into a buffer, choosing and
  pinning the direct candidate, capturing the damage snapshots;
- **submit** — the only part that forks, on the transport state of *that*
  output's device: `Legacy` takes today's call unchanged; `Owner` offers the
  prepared intent to the device's conductor and wakes it.

So the fixtures exercise the **real producer code**, and 2c-ii's test
`AdmissionSource` is replaced by a real source reading backend and scene state.
Rejected: a per-device route trait with a legacy and an owner implementation
(restructures the legacy paths of a 53-thousand-line `backend.rs` for no
additional guarantee), and owner producers written only in fixtures (evidence on
a path production never takes — the P3-2/P3-3 lesson).

`allows_legacy` stays as defence in depth: a legacy write that escapes the fork
on an `Owner` device is refused by the transport gate. It is **not** the proof
of exclusivity; section 6.3 is.

## 3. Architecture

**3.1. The real admission source.** `AdmissionSource`
(`kms/render/admission.rs:105`) was 2c-ii's injected stand-in. 2c-iii replaces
it with a source backed by the backend's own state: composed readiness from the
scanout pool and the prepared frames, direct readiness and eligibility from
section 5.1's predicate, the homogeneous group from the current topology,
`describe` from the prepared intents. Whether it stays a boxed trait object or
becomes methods on `KmsBackend` is the plan's decision, under 2c-ii §11's
constraint: the decider borrows nothing, and nothing holds a long-lived borrow
across a wake. The test source may survive for decider-level tests; no exit
criterion of this document may rest on it.

**3.2. Old-state registration — the production caller P3-2/P3-3 lacked.**
Verified on this branch at `9dcd5a1f`: the conductor builds the ledger with
`Submitted::new(old, new)` inside `begin_with_ledger`
(`admission.rs:709`/`:732` for primaries, `:830`/`:838` for direct), and
`register_commit_dependencies` (`resources/commit.rs:572`, the only caller of
`register_kms`) is still called only from tests. So in the conductor, as in
2c-i, a displaced buffer's `KmsRelease` obligation is never registered. **The
converted route must register the commit's old/new dependencies when it
dispatches**, so that each displaced buffer's release is discharged by that
commit's real completion evidence (C.0 §10.2), never by the page event. This is
the production caller whose absence stopped P3-2/P3-3 (debt spec §9.5), and
section 6.4 carries them. If a reason turns up in planning why the route cannot
register there, it is an F8 stop recorded here, not a silent omission.

**Reachability, verified at `13fa30bd` (user's request, before any plan):**

- *Registration — no production caller.* Every `KmsRelease` registration
  outside `register_kms` is test-only: the three in `store.rs` (`:2878`,
  `:2979`, `:4004`) sit inside `#[cfg(test)] mod tests` (`store.rs:1965`), and
  `register_commit_dependencies` is called only from `resources/tests.rs` and
  `guard_tests.rs`.
- *Discharge — already reachable.* `CommitConsumer::consume`
  (`resources/commit.rs:221`) discharges a commit's `kms_obligations` through
  `discharge_commit_kms_obligations` (`:523`) on `HardwareComplete` and
  `CompletionRetired`, and `consume` has production callers in the owner-event
  routing (`backend.rs:19821` and the following arms) and in
  `admission_consume_events` (`admission.rs:1215`). Today it discharges
  nothing, only because the ledger arrives with empty `kms_obligations`.
- *The fit.* `register_commit_dependencies` returns exactly the
  `Submitted<CommitResources>` the conductor's ledger closure builds, and
  `KmsBackend::resource_service` is a field disjoint from `platform` and
  `commit_consumer`, so the closure can borrow it.

**Two owner-API gaps the plan must close first** (both in
`owner/device.rs`):

1. `begin_with_ledger`'s closure is `FnOnce(CommitId) -> Submitted<R>`
   (`device.rs:1472`), infallible, while registration can fail (it returns the
   old and new resources on error, after cancelling what it registered). The
   `CommitId` exists only inside `begin`, so registration cannot move before it.
   Needed: a fallible ledger closure whose error makes `begin` release the slot
   and the reservation, with the resources handed back — and the conductor then
   `abort`s its token, consuming no fairness state (2c-ii §6).
2. `begin_with_ledger` refuses any description with `page_flip_event` or
   `present_consumers` (`device.rs:1480`), and the one public entry that accepts
   a `CompletionContext`, `begin_with_context` (`device.rs:1322`), takes the
   ledger **by value**, before the `CommitId` exists. The CommitId-aware
   closure form, `begin_with_context_and_ledger`, is private. A real
   Present-carrying commit therefore has no public entry that can register its
   dependencies. Needed: a public, fallible, CommitId-aware ledger entry that
   also takes the completion context.

Both are owner changes inside C.0's existing contracts (they add no state and
no outcome); C1's plan owns them, because composed is the first converted
producer.

**3.3. Owner events reach the consumers.** 2c-ii's `route_owner_event_batch` is
the only path owner events take (plan B2). 2c-iii adds consumers behind it: the
damage transactions (section 4.2) and the direct frame state (section 5).
Milestones are delivered by `CommitId`; a consumer that sees an unknown
`CommitId` ignores it and records a telemetry count, never guesses an owner.

## 4. Plan C1 — composed producer, damage transaction, bundles

**4.1. Producer.** The scene tick is unchanged up to the composed buffer and its
`PendingAck` (repaint, per-output damage, `drawable_snapshots`) — the prepare
half. The submit half is the primary write at `scene.rs:4683` (where
`WriterClass::Primary` is checked today):

- **`Legacy`:** today's flip, unchanged.
- **`Owner`:** the frame becomes the CRTC's **desired composed generation**
  (`admission_offer_composed`) and the conductor is woken. The buffer and its
  metadata are retained by the intent.

Latest-wins (2c-ii §3): **a composed generation displaced before admission
stages nothing and acks nothing**, and its buffer returns to the pool. Its
drawable snapshots were never acked, so they remain pending in the store and the
next peek includes them. Composed readiness is 2c-ii §4's: a reusable buffer not
retained by current, submitted or delayed-release ownership, and finished
producer waits (the render-completion stage, `InFlightStage`).

**4.2. The damage transaction** (C.0 §12.1, stage 2c §5). Created when the
conductor confirms the admission, identified by the `CommitId` together with
the exact set of `(output, buffer index, generation)` it includes, and carrying
each output's captured `PendingAck` contents. Retained before dispatch; nothing
staged before `Accepted`.

| Owner outcome | Damage action |
| --- | --- |
| `Dispatched`, `FailedBeforeSubmit`, a pre-IPC refusal | None. The transaction closes; the next tick recomputes an identical repaint |
| `Accepted` | Stage each painted buffer once (`commit_submitted`) — today this runs at submission, `scene.rs:4841`, and that is the line that moves |
| `HardwareComplete` | Apply (`retire_success`), ack the per-output captured drawable snapshots, subtract the captured structure/failed-repaint damage, push damage history, set `prev_presented` — what `handle_page_flip_complete` (`scene.rs:2129`) does today for the legacy route |
| `Presented` | None |
| `CompletionUnknown`; incarnation poison, recovery, topology, VT release, device loss | Invalidate |
| A post-accept failure with the prior state proven current | Restore. If no `TerminalState` expresses this case today, the plan records it as an F8 stop against C.0 §12.1's row rather than inventing one |

The composed **pool slot's release is separated from the ack**: it follows
2c-i's ledger (`CompletionRetired`, `PriorBufferReleased`, and the GPU fence gate
that exists today), not the damage milestone.

**4.3. Bundles (tier 5, DMG-4).** One transaction over every included output:
staged at the single `Accepted`, applied at the single `HardwareComplete`,
naming exactly the outputs of `ExpectedCompletionCrtcs`. No output is staged
twice without an intervening apply or invalidate; an output not represented
earns nothing. The conductor's bundle dispatch already exists for composed
members (`admission.rs`, `Admitted::Bundle`); C1 gives it real members.

**4.4. Scene contracts preserved** (stage 2c §5, "Current-master scene
contracts"). An invalidated or failed transaction owes a repaint independently
of fresh damage (`owes_repaint` feeds the scene and backend wake predicates);
`NoPieces` and `HiddenDamage` stay distinct; `walk_needed`,
`pending_presentation_for_output` and retained `last_pieces` are kept; non-empty
`Hidden`, `OtherOutput` and `OffOutput` snapshots authorize no ack; participating
outputs are captured before any ack; a device-owner wake does not by itself walk
every output. The v1.5.0 row for 2c-iii (`PaintTarget` coordinates and clips,
root `IncludeInferiors` snapshots with their own GPU lifetime) applies unchanged:
the conversion moves the flip, not the compose.

**4.5. C1 exit evidence** (fixtures with the real scene tick in `Owner`, the
real owner and the helper; each row has a named test and a named mutation):
every row of the table in 4.2; new paint between capture and `HardwareComplete`
survives; two outputs with permuted completions, bundled and separately
scheduled; a displaced generation acks nothing; invalidation without fresh paint
still repaints; skipped-output dormancy; off-output damage not acked; old-state
registration of section 3.2.

## 5. Plan C2 — direct producer

**5.1. The real eligibility predicate.** Today the inputs of direct eligibility
— VT, clock epoch, cursor, the resolved paint chain's `has_border_clip()` — are
computed inline in `try_present_direct` (`backend.rs:21205`) around
`scanout_direct_eligible` (`backend.rs:321`). They are extracted into one
predicate used by both routes: `Legacy` keeps calling it where it is, and the
real source answers `direct_eligible` with it. No predicate separate from
production's remains.

**5.2. Layout hooks.** Every change that can invalidate a direct decision
advances the conductor's layout generation (`admission_note_layout_change`):
at least border, geometry, storage relayout, redirect and topology changes. This
is **not** a closed list. **The plan's first task is the implementer's
enumeration of the real sites**, the approach that found every owner-event
producer in one pass in plan B2. Invariant (v1.5.0 table; 2c-ii round-2 B-2): a
successor queued as eligible whose ancestor then gains a border never reaches a
commit, including when retirement promotes it.

**5.3. Producer.** Prepare is shared: `try_present_direct` chooses the
candidate, pins it and imports the framebuffer. Submit forks:

- **`Legacy`:** `submit_direct_frame` (`backend.rs:2466`) and
  `submit_queued_direct_successor` (`backend.rs:2510`), unchanged.
- **`Owner`:** `admission_offer_direct` with the real frame, which occupies the
  latest-wins successor slot. **Retirement promotion no longer commits from the
  event handler** (C.0 §12): the conductor's wake admits it through tier 3 or
  tier 6. A displaced successor takes the never-submitted path 2c-ii already
  built (`managed_terminalize_queued_direct_successor`,
  `defer_direct_successor_skip` at `backend.rs:2562`): idle exactly once, `Skip`
  deferred behind the predecessor.

**5.4. F13b-D1.** The request builder takes the frame's present-pin leases —
source pin and fallback-target pin — **by value** and moves them into the
commit's `CommitResources`, as 2c-ii §1 prepared. Invariant: while the
dispatched direct commit lives, its pins are released by no path other than the
ledger (`PriorBufferReleased`, or the rejection/never-dispatched path). A builder
that constructs an empty lease set must fail a named test.

**5.5. DMG-5.** Entering direct invalidates every composed buffer of the
affected outputs; no milestone of a direct transaction applies composed damage.

**5.6. Cursor and gamma.** C2 adds no payloads: absorption exists since B1/B2 and
real producers are stage 4's. What is proven: a direct commit never carries an
unchanged cursor, and a primary flip event does not retire a newer cursor
generation (C.0 §12).

**5.7. C2 exit evidence** (fixtures with the real `try_present_direct` in
`Owner`): the successor that gains a border while queued, promoted or not;
retirement promotion ordered predecessor → `Skip` → admission → publication;
lease adoption; displacement with a deferred `Skip`; composed invalidation on
direct entry; eligibility identical between the two routes for the same inputs.

## 6. Plan C3 — unflip, multi-device, route selection, hardware

**6.1. Ready unflip (tier 2).** `decision_requires_unsupported`
(`admission.rs:65`) aborts `Unflip` today; C3 dispatches it. Ready (2c-ii §4)
when the exit-retirement position is free, every affected output has its
retained composed framebuffer, and the direct shadow is materialized. The
request replaces the complete plane set in one transaction, as
`submit_composed_unflip` (`backend.rs:3008`) does, because AMD rejects
per-CRTC replacement with `ENOSPC`. On an `Owner` device the reasons that reach
`request_direct_unflip` (`backend.rs:2219`) — cursor, overlay and topology
invalidation, a failed successor send — enter `admission_request_unflip`.
Returning to composed invalidates the affected composed buffers (DMG-5). The
unflip must not drop or flash the cursor and preserves the current gamma
(C.0 §12). `Topology` and `CursorRecovery` stay `Unsupported`: stages 3 and 4.

**6.2. Multi-device.** One conductor per `DrmDeviceKey`
(`admission_conductors`), each with its own admission, layout generation and
transport state. A grouped direct unit never crosses devices
(`direct_scanout_topology_eligible`, `backend.rs:3161`). Invariant: an event,
wake, refusal or bound violation on one device changes nothing on another.

**6.3. Route selection and exclusivity.** Each submit site of sections 4.1, 5.3
and 6.1 — composed, direct, retirement promotion, unflip — reads the transport
state of its own device. Invariant: on an `Owner` device no primary or unflip
legacy write is issued. Each site carries its own mutation ("force the legacy
branch"), which must break a named test; the transport gate refusing that write
is **not** accepted as the test's observation, because it is the defence the
fork is supposed not to need. On a `Legacy` device behaviour is identical to
today, proven by the full software gate and the hardware gate.

**6.4. The hardware test.** From tty2 on card1, asking the user first (GPU in
personal use). A fixture establishes `Owner` with the writer-coverage evidence
of the debt spec §4.4, then drives, with the real conductor, helper and
producers:

1. a composed frame: `Accepted` → `HardwareComplete` → damage applied;
2. a direct frame from a Vulkan-rendered PRIME-imported buffer (as part 3's
   P3-1): out-fence `Success`, leases released at `PriorBufferReleased`;
3. the unflip back to composed.

Because section 3.2 gives `KmsRelease` a production caller, this test also
carries **P3-2** (a displaced buffer's `KmsRelease` is discharged by the real
completion) and **P3-3** (a retained buffer registers none). Before the plan
anchors them, the plan's author re-runs the reachability check on the
implemented C1/C2 code: a non-test caller of `register_kms` must exist on the
path the test drives. If it does not, they stay the debt spec's §9.5 F8 for
stages 3/4, and this document says so.

## 7. Out of scope, and the doors that must stay open

- **Production activation** — stages 3/4 complete the writer coverage; the
  selection boundary of section 2.3 is where they switch a device to `Owner`.
- **The cursor coordinate lane** (C.0 §7.1) — never absorbed as coordinate-only
  intent (2c-ii §8).
- **Phase C.1** — the direct successor slot remains the one C.1 extends; nothing
  in the direct producer may assume a synchronous successor in a way that
  prevents the async variant (2c-ii §8).
- **Lifecycle unflip and modeset callers** — stage 3 (C.0 §18); only the
  primary-restoration unflip is converted here (stage 2c §6).

## 8. Verification

### 8.1. Level

Everything is proven at fixture level with the real producers, the real
conductor and the real owner, plus the single hardware test of section 6.4.
Every finding and status line states which. No criterion rests on 2c-ii's test
source (section 3.1).

### 8.2. Exit criteria

Each has a named test and a named mutation that must break it; mutations are
confirmed to have compiled and to remove the behaviour, and are applied by line,
not by the first textual match.

| Criterion (source) | Mutation that must fail it |
| --- | --- |
| Staging at `Accepted`, not at dispatch (DMG-1) | Stage at submission, as the legacy route does |
| Apply at `HardwareComplete`, never at `Accepted` or `Presented` (DMG-2) | Apply at `Presented` |
| Unknown and invalidation events invalidate (DMG-3) | Restore on `CompletionUnknown` |
| A bundle is one transaction; no double staging (DMG-4) | Stage one bundle output at a separate milestone |
| Direct entry and unflip return invalidate composed buffers (DMG-5) | Drop the invalidation on direct entry |
| A displaced composed generation acks nothing (4.1) | Ack the displaced generation's snapshots |
| Damage after capture survives the ack (stage 2c §5) | Ack from the live store instead of the captured snapshots |
| Pool release follows the ledger, not the ack (4.2) | Release the pool slot at `HardwareComplete` |
| Old-state dependencies registered at dispatch (3.2) | Build the ledger without registering |
| One eligibility predicate for both routes (5.1) | Let the owner route skip the border-clip input |
| A border gained while queued is never committed, promoted or not (5.2) | Skip the layout hook at one enumerated site |
| Retirement promotion goes through the conductor, in order (5.3) | Commit the successor from the event handler |
| Leases carried by value (F13b-D1, 5.4) | Build an empty lease set |
| Ready unflip dispatched as one full-plane-set transaction (6.1) | Dispatch one CRTC's plane alone |
| Device isolation (6.2) | Route one device's event to another's conductor |
| Legacy exclusion per submit site (6.3) | Force the legacy branch at that site |

### 8.3. Plans and process

Three plans, C1 → C2 → C3 (section 2.2), each reviewed by codex through
`docs/superpowers/review/review.sh` before implementation and implemented by
codex one task per run; the coordinator verifies each task's full gate and runs
the mutations. Each plan is written only after the previous one is accepted, so
it cites implemented interfaces. Each plan carries a `docs/status.md` step.
This document itself is reviewed by codex before plan C1 is written.

### 8.4. Gate

`cargo +nightly fmt`; `cargo clippy --all-targets -- -D warnings` in the default
build and with `--features tcp-transport` and `--features xdmcp`; the
deterministic suites (the helper-spawning ones run several times — one green run
is not evidence); `cargo check` for Linux glibc, Linux musl and FreeBSD. The
full hardware gate (`render_acceptance`, `c0_2ci -- --ignored`, the library's
other ignored tests) after each plan, and section 6.4's test after C3 — each
only after asking the user.

## 9. Questions for the plans

Placement and plumbing only; none can change the decisions above.

- C1: where the prepared composed frame lives between the tick and admission,
  and how its buffer's pool phase expresses "retained by a desired intent".
- C1: which `TerminalState` (if any) is the post-accept failure with proven
  prior state (section 4.2's last row).
- C2: the extracted predicate's signature, and whether `Legacy` callers move to
  it in C2 or keep an adapter.
- C3: how the retained composed framebuffer and the shadow materialization
  report readiness without a borrow across the wake.
- C3: whether the hardware fixture reuses part 3's tty2 fixture
  (`part3_tests.rs`) or needs its own.
