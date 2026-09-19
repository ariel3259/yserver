# Phase C.0 stage 2c-iii — primary conversion and damage

**Status:** design, revision 4 (2026-09-19). Its three design sections (plans
Ci, Cii and Ciii — here sections 4, 5 and 6) were approved one by one with the user
in brainstorming, together with three decisions recorded in section 2: the
evidence level, the split into three plans with the composed plan first, and
the prepare/submit selection boundary.
Revision 2 incorporates codex round 1
(`../findings/2026-09-19-stage-2c-iii-design-review-round1.md`: 2 blocking,
3 major, all verified against the tree and accepted): the damage transaction
survives `Dispatched` (B-1), the registration caller is a hard requirement with
no deferral (B-2), Present carriage from producer to owner (M-1, new section
3.3), each pool-release gate tested on its own (M-2), and DMG-5 on the unflip
return (M-3).
Revision 3 incorporates codex round 2
(`../findings/2026-09-19-stage-2c-iii-design-review-round2.md`: 2 blocking,
1 major, all verified and accepted): composed commits are non-Present
primaries and composited Presents keep their GPU-completion authority, so
section 3.3's carriage is direct-only and the Present-carrying owner entry moves
to Cii (B-1); the damage transaction is installed inside the ledger closure,
before any event can be routed (B-2); the pool-release gates are named in code
terms and each is dropped alone by one mutation (M-1). The plans, called C1,
C2 and C3 up to revision 3's first commit, are now **Ci, Cii and Ciii** (user,
2026-09-19), so they cannot be read as Phases C.1 and C.2, which follow C.0.
The round-1 and round-2 findings keep the old names. Implementation plans
follow (section 8.3).
Revision 4 incorporates codex round 3
(`../findings/2026-09-19-stage-2c-iii-design-review-round3.md`, instrument
`da807b70` with 24 excerpts: 1 blocking, 1 major, both verified and accepted):
the direct frame state is the single Present authority and `present_consumers`
holds CRTCs only (B-1); a retained-allocation hardware case and a mutation per
P3 invariant (M-1).

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
| Real `CommitDescription` builders and producer readiness | Ci (composed), Cii (direct), Ciii (unflip) |
| The real direct-eligibility predicate, extracted from `try_present_direct` | Cii |
| Real layout-change hook sites | Cii |
| F13b-D1: dispatched `CommitResources` carries the present-pin leases by value | Cii |
| Ready-unflip dispatch (needs a retained composed framebuffer) | Ciii |
| Multi-device conductor state | Ciii |
| The copied composed route (`platform.rs:6205`), whose flip follows a copy into a destination buffer | Ciii (section 6.3; user, 2026-09-19) |

## 2. Decisions taken in brainstorming

**2.1. Evidence level (user, 2026-09-19).** Fixture level, as in 2c-ii, **plus
one hardware test** from tty2 that puts card1 in `Owner` and drives a composed
frame, a direct frame and an unflip through the real conductor, the real helper
and the real producers (section 6.4). It is the first evidence that the owner
route works on a real device; the precedent is part 3 of the 2c-i debt stage.
Activating production was rejected: it contradicts stage 2c §6 and R8.

**2.2. Three plans, composed first (user, 2026-09-19).** Ci — composed producer,
damage transaction, bundles. Cii — direct producer, eligibility, layout hooks,
F13b-D1, retirement promotion. Ciii — unflip, multi-device, route selection,
hardware test. Composed goes first because it is the base traffic every other
path returns to, and damage is the contract with the most risk. Keeping damage
in Ci, rather than a plan of its own, avoids an intermediate state in which the
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
section 6.4 carries them. **This is not deferrable (round-1 B-2).** "Production
caller" means a call in non-test code on the converted `Owner` submit path of a
real producer — the path section 2.3's fork selects, driven by the fixtures
because production stays `Legacy` (R8) — not a test helper. Its absence on that
path blocks the acceptance of Ci (composed), Cii (direct), Ciii (unflip) and of
2c-iii; it cannot be handed to stages 3/4. If planning finds a reason the route
cannot register, that is an F8 stop that halts the stage and goes back to the
user, not a deferral.

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
no outcome). Gap 1 is Ci's (section 4.0), because composed is the first
converted producer. Gap 2 is Cii's (section 5.0): composed commits carry no
Present (section 3.3), so the direct producer is the first caller that needs a
Present-carrying entry (round-2 B-1).

**3.3. Present carriage from producer to owner (round-1 M-1, round-2 B-1).**
Each Present request has exactly one terminalization authority, and the
conversion does not move it.

- **Composited Presents keep their GPU-completion authority; composed commits
  are non-Present primaries.** Today a Present that is copied into a drawable
  and composited completes when its render work completes, not at a KMS flip:
  its `CompletedPresentEvent`s are held in `OpenFrame.pending_present_completions`
  and, after a successful submission, move into a `PendingPresentBatch` behind
  the exported fence (`kms/render/engine.rs:2777`–`2821`, Phase B.3). The scene
  sees only a Boolean per output (`pending_presentation_for_output`,
  `scene.rs:2013`), with no request identity. So a composed `CommitDescription`
  carries **no** `present_consumers` and no `page_flip_event`, needs only
  `Accepted` and `HardwareComplete`, and **cannot manufacture Present
  completion** (C.0 §10.2). Displacing a composed generation changes nothing
  for those Presents: they were already completed or queued by the GPU batch.
  Moving composited Presents onto the KMS commit would be a protocol change
  (C.0 §12 preserves Phase A+B's outcomes), not a 2c-iii conversion.
- **Direct commits carry their Present into the owner** (Cii). A direct commit
  that completes Present requests carries them, or the client's FIFO stays
  parked while every damage and resource test passes. Today the conductor sets
  neither `page_flip_event` nor `present_consumers` on any `CommitDescription`
  (`kms/owner/build.rs:27`), because `begin_with_ledger` refuses both.
  Invariants for the direct producer (revised by round-3 B-1):

  - **two different things, never confused.** `CommitDescription::present_consumers`
    is a set of **CRTC ids** — the members of the kernel event set whose page
    event supplies MSC/UST for this commit (`owner/build.rs:27`; the closure
    rejects a consumer outside the event set, `owner/closure.rs:215`). It
    never carries a Present serial, FIFO position or notification. The
    **protocol payload** — the `CompletedPresentEvent` with its serial, target
    and idle/notify state — is not in the description at all;
  - **exactly one terminalization authority per direct Present: the direct
    frame state.** The frame carries its event from preparation
    (`DirectPresentFrame`), and the conductor's confirmation already moves it
    intact into the accepted slot (`managed_confirm_direct_dispatch`,
    `backend.rs:2620`). Confirmation also binds that accepted frame to the
    commit's `CommitId`. The owner does not complete the Present: its
    `Presented { samples }` for that `CommitId` delivers the MSC/UST sample to
    the bound frame, and the frame's single publication at retirement
    (`managed_enqueue_retired_direct_completion`, `backend.rs:2638`) is the
    **only** CompleteNotify/FIFO wake that request gets. No other consumer —
    the damage transaction, the resource consumer, the conductor — publishes
    or wakes for it;
  - the description built for the admitted direct generation sets
    `page_flip_event` and names the CRTCs of that event set in
    `present_consumers`, with the `CompletionContext`, through section 5.0's
    entry, so that the owner correlates and validates the page event the sample
    comes from;
  - Present terminalization stays independent of damage and resource release
    (C.0 §10.4, stage 2c §3): the retirement publication is `Flip` with the
    validated `Presented` sample for that `CommitId`; an accepted Present
    lacking validated presentation terminalizes as `Skip` with the last
    validated clock sample, never a fabricated timestamp; no idle or release is
    emitted before the ledger proves it;
  - a Present whose generation is displaced before admission keeps the
    frame-owned never-submitted path 2c-ii built (idle once, `Skip` deferred);
    its deferred `Skip` is published only after the predecessor's own
    retirement publication — the owner-bound frame above — or at once when no
    predecessor is in flight
    (`managed_publish_deferred_successor_skips_if_no_predecessor`). It is not
    carried by the displacing generation's commit.

**3.4. Owner events reach the consumers.** 2c-ii's `route_owner_event_batch` is
the only path owner events take (plan B2). 2c-iii adds consumers behind it: the
damage transactions (section 4.2) and the direct frame state (section 5).
Milestones are delivered by `CommitId`; a consumer that sees an unknown
`CommitId` ignores it and records a telemetry count, never guesses an owner.

## 4. Plan Ci — owner entry, composed producer, damage transaction, bundles

**4.0. The owner entry for a registered ledger** (section 3.2's first gap).
Ci's first tasks, before any producer is converted, because the composed
producer is the first real caller. Stated as invariants; the shape of the API is the plan's:

- **A failed registration leaves nothing behind.** When the ledger closure
  fails, `begin` returns an error and the owner is as it was before the call:
  no live record, the slot and the reservation released, no event emitted for
  that commit. Every obligation the registration had taken is cancelled, and
  the old and new resources come back to the caller intact. The conductor then
  `abort`s its token, so no ticket is spent, no turn advances and no loser ages
  (2c-ii §6). This is a `begin` refusal, not a pre-IPC `send_on` refusal: no
  record reaches `NeverDispatched`.
- **The obligations name the record's own commit.** Each registered
  `KmsRelease` is keyed by the `CommitId` of the record that carries it, so the
  discharge in `consume` matches it (`record_kms_discharged` and
  `discharge_commit_kms_obligations` compare the commit).
- **The existing entries keep their refusals.** `begin_with_ledger` still
  refuses a Present-carrying description; nothing widens it silently.

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

**4.2. The damage transaction** (C.0 §12.1, stage 2c §5). Identified by the
`CommitId` together with the exact set of `(output, buffer index, generation)`
it includes, and carrying each output's captured `PendingAck` contents.
**Installed inside the CommitId-aware ledger closure of section 4.0** — the
first point where the `CommitId` exists, and before any IPC (round-2 B-2). The
owner events that `begin` and `send_on` return are routed only after it is
installed, and admission is confirmed after the send, as 2c-ii §6 requires. If
the closure fails, nothing is installed; if `send_on` refuses before IPC, the
`NeverDispatched` row below closes it. So no milestone of the commit can reach a
consumer before its transaction exists, and section 3.4's rule for an unknown
`CommitId` never discards one. Nothing is staged before `Accepted`.

| Owner outcome | Damage action |
| --- | --- |
| `Dispatched` (`Submitting`) | None, and the transaction is **retained**: `Dispatched` is not terminal and precedes `Accepted` (C.0 §10.2) |
| `FailedBeforeSubmit`, a pre-IPC refusal (`NeverDispatched`) | None. The transaction closes without staging; the next tick recomputes an identical repaint |
| `Accepted` | Stage each painted buffer once (`commit_submitted`) — today this runs at submission, `scene.rs:4841`, and that is the line that moves |
| `HardwareComplete` | Apply (`retire_success`), ack the per-output captured drawable snapshots, subtract the captured structure/failed-repaint damage, push damage history, set `prev_presented` — what `handle_page_flip_complete` (`scene.rs:2129`) does today for the legacy route |
| `Presented` | None |
| `CompletionUnknown`; incarnation poison, recovery, topology, VT release, device loss | Invalidate |
| A post-accept failure with the prior state proven current | Restore. If no `TerminalState` expresses this case today, the plan records it as an F8 stop against C.0 §12.1's row rather than inventing one |

The composed **pool slot's release is separated from the ack** and waits for
three independent gates (round-2 M-1), none of which is the damage milestone:

1. **`CompletionRetired`** has handed the displaced buffer from the commit to
   the consumer's retiring state (`CommitConsumer::consume`);
2. **`PriorBufferReleased`** holds, which in this code is not an owner event:
   it is the resource service reporting the buffer's allocation free, its
   `KmsRelease` obligation discharged by the displacing commit's completion
   (section 3.2; C.0 §10.2 item 6);
3. the compose **GPU fence** of that buffer has signalled (the gate
   `handle_page_flip_complete` applies today).

**4.3. Bundles (tier 5, DMG-4).** One transaction over every included output:
staged at the single `Accepted`, applied at the single `HardwareComplete`,
naming exactly the outputs of `ExpectedCompletionCrtcs`. No output is staged
twice without an intervening apply or invalidate; an output not represented
earns nothing. The conductor's bundle dispatch already exists for composed
members (`admission.rs`, `Admitted::Bundle`); Ci gives it real members.

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

**4.6. Decided while planning Ci (user, 2026-09-19).**

- **Evidence with the real tick, under Vulkan.** Every Ci criterion that
  involves the composed producer, the damage transaction or buffer reuse is
  proven with the real scene tick, in `#[ignore]`d `_vulkan` fixtures. The
  implementer's sandbox has no `/dev/dri`, so the coordinator runs them, and
  their mutations, after asking the user each time (the GPU is in personal
  use). Criteria with no tick in them — the owner entry of section 4.0 and the
  conductor's registration — stay deterministic.
- **The copied route is Ciii's.** Ci converts the shared managed composed route
  only. Until Ciii converts the copied route, a device with an output on the
  copied route, or on an unmanaged scanout pool, **cannot enter `Owner`**; it
  stays `Legacy`, so section 6.3's exclusivity holds on every `Owner` device.
- **Producer fences do not cross the ioctl** (C.0 §10.2 item 3). The legacy
  composed flip hands the render fence to KMS as `IN_FENCE_FD`
  (`drm/page_flip.rs:152`); the owner route does not. A composed generation is
  `Ready` only once its render completion has signalled, observed through the
  platform's existing render-completion drain.
- **Old state is per member** (verified defect of the 2c-ii conductor). The
  conductor takes the device's **whole** current state as the commit's old
  state (`CommitResourceConsumer::take_current`), and `CompletionRetired` moves
  all of it to releasing and makes the new state the whole current state
  (`resources/commit.rs:160`, `:262`–`:295`). A composed commit for one CRTC
  would release another CRTC's on-screen buffer. 2c-ii never saw it because its
  fixtures have one CRTC. Ci makes the old state exactly the current resources
  of the members the commit covers; every other member's current state stays
  current.

**4.5. Ci exit evidence** (fixtures with the real scene tick in `Owner`, the
real owner and the helper; each row has a named test and a named mutation):
every row of the table in 4.2; new paint between capture and `HardwareComplete`
survives; two outputs with permuted completions, bundled and separately
scheduled; a displaced generation acks nothing; invalidation without fresh paint
still repaints; skipped-output dormancy; off-output damage not acked; old-state
registration of section 3.2; a composed commit carries no Present and a
composited Present still completes exactly once, from its GPU batch (section
3.3); a milestone returned by `begin`/`send_on` finds its transaction installed
(section 4.2); the invariants of section 4.0, including a
registration failure that leaves the owner and the decider untouched.

## 5. Plan Cii — direct producer

**5.0. The Present-carrying owner entry** (section 3.2's second gap; moved
from Ci by round-2 B-1). Cii's first task. A description with `page_flip_event`
or `present_consumers` is begun through a public entry that takes its
`CompletionContext` **and** section 4.0's CommitId-aware, fallible ledger
closure, with section 4.0's failure invariants. That entry applies every check
`begin_with_context` applies today; it is not a way around them.

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

**5.6. Cursor and gamma.** Cii adds no payloads: absorption exists since B1/B2 and
real producers are stage 4's. What is proven: a direct commit never carries an
unchanged cursor, and a primary flip event does not retire a newer cursor
generation (C.0 §12).

**5.7. Cii exit evidence** (fixtures with the real `try_present_direct` in
`Owner`): the successor that gains a border while queued, promoted or not;
retirement promotion ordered predecessor → `Skip` → admission → publication;
lease adoption; displacement with a deferred `Skip`; composed invalidation on
direct entry; eligibility identical between the two routes for the same inputs;
section 3.3's Present carriage on the real direct producer, with
`HardwareComplete`/`Presented` in both orders, a missing `Presented`, and no idle
or release before the ledger proves it; section 5.0's entry and its checks.

## 6. Plan Ciii — unflip, multi-device, route selection, hardware

**6.1. Ready unflip (tier 2).** `decision_requires_unsupported`
(`admission.rs:65`) aborts `Unflip` today; Ciii dispatches it. Ready (2c-ii §4)
when the exit-retirement position is free, every affected output has its
retained composed framebuffer, and the direct shadow is materialized. The
request replaces the complete plane set in one transaction, as
`submit_composed_unflip` (`backend.rs:3008`) does, because AMD rejects
per-CRTC replacement with `ENOSPC`. On an `Owner` device the reasons that reach
`request_direct_unflip` (`backend.rs:2219`) — cursor, overlay and topology
invalidation, a failed successor send — enter `admission_request_unflip`.
Returning to composed invalidates every affected composed buffer, and each is
repainted in full before it is scanned out again (DMG-5); a Ciii fixture proves
it per output (round-1 M-3). The
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
3. the unflip back to composed;
4. **a commit whose new state retains an allocation of the old state for the
   same member** (round-3 M-1) — for example, a direct Present of the same
   source buffer again; the plan chooses the shape, and if no such commit is
   reachable on card1 it reports that as an F8 stop rather than substituting a
   fixture. The retained allocation gets no `KmsRelease` obligation and is not
   released, while any displaced allocation in the same commit is.

Because section 3.2 gives `KmsRelease` a production caller, this test also
carries **P3-2** (a displaced buffer's `KmsRelease` is discharged by the real
completion — steps 1–3) and **P3-3** (a retained buffer registers none — step
4). As the debt spec §9.3 requires, each has its own mutation run on the
hardware under the same filter: dropping the displaced buffer's registration
must fail P3-2, and registering the retained allocation must fail P3-3. Before the plan
anchors them, the plan's author re-runs the reachability check on the
implemented Ci/Cii code: a non-test caller of `register_kms` must exist on the
path the test drives. By section 3.2 that caller is already a condition of
Ci's and Cii's acceptance, so its absence here means an acceptance was wrong:
the stage stops and the defect is reopened, it is not deferred (round-1 B-2).
P3-2/P3-3 close the debt spec's §9.5 F8.

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
| Direct entry invalidates composed buffers (DMG-5) | Drop the invalidation on direct entry |
| The unflip return invalidates and repaints every affected composed buffer (DMG-5; round-1 M-3) | Drop the invalidation on the unflip return; invalidate only one affected output |
| A displaced composed generation acks nothing (4.1) | Ack the displaced generation's snapshots |
| Damage after capture survives the ack (stage 2c §5) | Ack from the live store instead of the captured snapshots |
| Pool release follows the ledger, not the ack (4.2) | Release the pool slot at `HardwareComplete` |
| Pool release waits for **every** gate of section 4.2, each withheld alone (round-1 M-2, round-2 M-1) | Three mutations, each dropping exactly one gate: release without `CompletionRetired`; release with the `KmsRelease` obligation still outstanding; release before the GPU fence signals |
| A composed commit carries no Present; composited Presents complete once, from the GPU batch (3.3; round-2 B-1) | Attach a composited Present to the composed description; complete it at `HardwareComplete` as well |
| The damage transaction exists before any of its milestones is routed (4.2; round-2 B-2) | Install the transaction at `confirm`, after the returned events are routed |
| Direct Present: one authority, the owner-bound frame; `present_consumers` holds CRTCs only; exactly one publication (3.3; round-1 M-1, round-3 B-1) | Also publish from an owner-event consumer (a second CompleteNotify); put a Present serial in `present_consumers`; publish the retirement completion without the bound `Presented` sample |
| Direct Present requests reach the owner and terminalize independently (3.3; round-1 M-1) | Drop `present_consumers` from the built description; fabricate a `Flip` timestamp for a missing `Presented`; idle before the ledger proves release |
| Old-state dependencies registered at dispatch (3.2) | Build the ledger without registering |
| A retained allocation registers no obligation and is not released, on real hardware (6.4 step 4; P3-3; round-3 M-1) | Register the retained allocation as if displaced |
| A displaced allocation is released only by the real completion, on real hardware (6.4; P3-2) | Drop the displaced allocation's registration |
| A failed registration leaves the owner as before `begin` (4.0) | Keep the slot reserved on a ledger error |
| A failed registration consumes no admission state (4.0; 2c-ii §6) | `confirm` instead of `abort` after a ledger error |
| A Present-carrying commit registers through the context entry, with its checks (5.0) | Skip the completion-context validation in the new entry |
| Registered obligations carry the record's own `CommitId` (4.0) | Register under a different `CommitId` than the record's |
| `begin_with_ledger` still refuses Present-carrying descriptions (4.0, 5.0) | Drop the `page_flip_event`/`present_consumers` refusal |
| One eligibility predicate for both routes (5.1) | Let the owner route skip the border-clip input |
| A border gained while queued is never committed, promoted or not (5.2) | Skip the layout hook at one enumerated site |
| Retirement promotion goes through the conductor, in order (5.3) | Commit the successor from the event handler |
| Leases carried by value (F13b-D1, 5.4) | Build an empty lease set |
| Ready unflip dispatched as one full-plane-set transaction (6.1) | Dispatch one CRTC's plane alone |
| Device isolation (6.2) | Route one device's event to another's conductor |
| Legacy exclusion per submit site (6.3) | Force the legacy branch at that site |

### 8.3. Plans and process

Three plans, Ci → Cii → Ciii (section 2.2), each reviewed by codex through
`docs/superpowers/review/review.sh` before implementation and implemented by
codex one task per run; the coordinator verifies each task's full gate and runs
the mutations. Each plan is written only after the previous one is accepted, so
it cites implemented interfaces. Each plan carries a `docs/status.md` step.
This document itself is reviewed by codex before plan Ci is written.

### 8.4. Gate

`cargo +nightly fmt`; `cargo clippy --all-targets -- -D warnings` in the default
build and with `--features tcp-transport` and `--features xdmcp`; the
deterministic suites (the helper-spawning ones run several times — one green run
is not evidence); `cargo check` for Linux glibc, Linux musl and FreeBSD. The
full hardware gate (`render_acceptance`, `c0_2ci -- --ignored`, the library's
other ignored tests) after each plan, and section 6.4's test after Ciii — each
only after asking the user.

## 9. Questions for the plans

Placement and plumbing only; none can change the decisions above.

- Ci: where the prepared composed frame lives between the tick and admission,
  and how its buffer's pool phase expresses "retained by a desired intent".
- Ci: which `TerminalState` (if any) is the post-accept failure with proven
  prior state (section 4.2's last row).
- Cii: the extracted predicate's signature, and whether `Legacy` callers move to
  it in Cii or keep an adapter.
- Ciii: how the retained composed framebuffer and the shadow materialization
  report readiness without a borrow across the wake.
- Ciii: whether the hardware fixture reuses part 3's tty2 fixture
  (`part3_tests.rs`) or needs its own.
