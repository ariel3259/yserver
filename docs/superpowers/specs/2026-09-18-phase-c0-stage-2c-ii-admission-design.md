# Phase C.0 stage 2c-ii — bounded intents and admission

**Status:** design, revision 1 (2026-09-18). Its five design sections (units,
readiness, the decision, the conductor, verification — here sections 3–7 and 10)
were approved one by one with the user in brainstorming, and the wlroots
comparison (section 9) was requested there; the written whole has not yet been
reviewed.
Codex review to follow, then two implementation plans (section 10.3).

**Authority**, most general first. This document elaborates the 2c-ii block; it
does not replace or relax any of them.

1. [C.0 specification](2026-08-26-phase-c0-atomic-kms-migration-design.md): §9 in
   full — the `SCHED` requirements (§9.1 bounded primary intents, §9.2 ordering
   classes, §9.2.1 fair admission and the starvation bound, §9.3 cursor and gamma
   progress, §9.4 `EBUSY`) — and §10 for the accepted-commit lifecycle.
2. [Stage 2c design](2026-09-08-phase-c0-stage-2c-conversions-and-damage-design.md):
   §2 (the 2c-i/2c-ii/2c-iii split, inputs, deliverable, exit evidence), §4 (the
   admission contract), §6 (activation), §7 (verification).
3. [Stage 2c-i design](2026-09-08-phase-c0-stage-2c-i-resource-terminalization-design.md),
   §6: the physical capacity 2c-ii consumes — the six direct-resource roles and
   the rule that normal admission waits on release capacity.

Where this document names *how*, it is new; where it names *what*, it cites the
section above that already requires it. If turning C.0 into code exposes
something in C.0 that does not hold, it is recorded here as an explicit
correction or F8 stop, never as a silent divergence (the precedent is the
2c-i debt spec's section 9.5).

## 1. What 2c-ii delivers, and what it does not

From the stage 2c design, §2: **bounded intents; one deterministic admission
decision including the exact absorbed generations; fairness bookkeeping.** Exit
evidence: the seven tiers, supersession bounds, no dispatch before source
readiness, and retirement promotion ordering — section 10.

2c-ii does **not**:

- add a second resource ledger (stage 2c §2) — resources stay with 2c-i;
- consume damage — 2c-iii;
- convert producers — 2c-iii; in 2c-ii intents come from tests;
- implement cursor/gamma producers or the cursor coordinate transport — stage 4
  (stage 2c §6);
- implement any Phase C.1 async behaviour (section 9);
- activate anything in production (R8): the production transport stays `Legacy`.

**Carried items.** F13c-m1 and F13c-m2, which the 2c-i F-13c review assigned to
2c-ii's spec, are **closed**: session 2 of the 2c-i debt stage bound pool-husk
accounting to an identity-bearing registration that fails closed when dropped or
skipped (`a87601c2`). F13b-D1 belongs to 2c-iii, per the F-13b review.

## 2. Architecture

Two parts, with a strict boundary.

- **The decider** — a new module in `kms/owner`. Pure: it holds only
  *descriptors* of intents (generations, readiness, tickets, turns), owns no
  resource and performs no I/O. Given a readiness snapshot it returns an
  `AdmissionDecision`.
- **The conductor** — one `AdmissionConductor` per device, on the platform side.
  The only component that talks to all three of: the decider, the
  `DeviceCommitOwner` (`begin`/`dispatch`), and 2c-i's resources (to build the
  snapshot and to carry out what the decider decides).

The file the conductor lives in is the plan's decision; this document fixes the
responsibility boundary. The decider being pure is what lets every rule in
section 5 be tested deterministically and in isolation, and it keeps the slot
model replaceable: a future per-CRTC design would change the decider's slot
model, not its callers (section 11).

## 3. Units, intents and storage bounds

**Primary ownership units** follow 2c-i §6 and C.0 §9.1. Two shapes exist today:

- **Composed** — one per CRTC; each output has its own composed desired state.
- **Grouped direct** — **one per DRM device**, covering its exact homogeneous
  output set. A shared source does not multiply the unit by CRTC. Any future
  partial or disjoint direct ownership needs its own bounded representation
  before admission (C.0 §9.1).

Round-robin and fairness are computed **per unit** (section 5 records how this
maps onto C.0's per-CRTC wording). This answers the fourth question stage 2c §7
left open — how grouped direct frames map to per-device admission: one grouped
unit is admitted as a whole, while per-output evidence and the shared source's
release stay with 2c-i's ledger, which does not release a shared source at the
first output.

**What the decider holds per unit** — descriptors, never resources:

| Slot | Content | Replacement |
| --- | --- | --- |
| Composed desired | Monotonic scene/damage generation and its readiness | Newest wins; never a queue of rendered frames |
| Direct successor | Source allocation generation and its readiness | Latest-wins; returns the displaced generation so the conductor runs it through 2c-i's never-submitted path |
| Unflip/recovery barrier | A request to restore the desktop | **Never** replaced; displaces incompatible unsent direct work (C.0 §9.1) |

**Per CRTC, maintenance:** one desired cursor and one desired gamma, each with a
device-monotonic `AdmissionTicket` that **survives payload replacement**, and an
aged flag (C.0 §9.2.1). Admitting or absorbing an identity consumes its ticket
exactly once; a newer update arriving while that identity is submitted gets a
**new** ticket and ages normally. **Per device:** whether a topology/lifecycle
barrier is waiting. Remapping or dropping tickets across a topology transition
is topology work, not 2c-ii's; 2c-ii only guarantees that a barrier never resets
a surviving ticket (C.0 §9.2.1).

**Resources do not live here.** The direct successor's leases are held by 2c-i's
`Successor` role; composed buffers by the existing pools; deferred `Skip`s by
2c-i's protocol ledger. The decider knows only *which generation* occupies each
slot.

**Bounds, each checkable:** per unit at most one composed, one successor and one
barrier; per CRTC at most one cursor and one gamma; per device at most one live
transaction — which is the owner's slot, not duplicated by the decider.

## 4. Readiness

Nothing is dispatched before its source is ready and its capacity reservations
are available (C.0 §9; 2c-i §6). An intent that is not ready **keeps its slot**
and stays latest-wins, but does not compete; the device slot remains free for
other eligible work.

**The decider queries nothing.** On each wake the conductor builds a
`ReadinessSnapshot` giving each descriptor as `Ready` or `Waiting(reason)`. The
reason serves telemetry and tests, never the decision.

| Intent | Ready when |
| --- | --- |
| Composed | A reusable buffer exists in the pool — not retained by current, submitted or delayed-release ownership — and the producer's waits have finished (2c-i §6) |
| Direct successor | Its source is ready (its pre-submit waits) **and** the ordinary-retirement position is free for the current state it will displace (2c-i §6); if that position is occupied it stays latest-wins but is not ready |
| Unflip barrier | The **exit**-retirement position is available and the composed-return path — retained composed framebuffers, shadow materialization, waits — is established for every affected output (2c-i §6); it does not need the ordinary position |
| Cursor / gamma | The payload is **compatible** with the current snapshot: its generations are still valid. A stale or incompatible payload cannot be absorbed (C.0 §9.2.1) |
| Topology barrier | A lifecycle/topology request is waiting |

**Wakes, and no timers.** Admission runs only when something concrete happens:
a new intent; a source wait finishing; **release evidence** (a 2c-i role freed,
a buffer returned to its pool); a retirement (`OwnerEvent::CompletionRetired`);
a barrier set or cleared. There is **no retry on capacity pressure** (2c-i §6):
when nothing is ready, nothing is scheduled, and the next real wake re-evaluates.

**Compatibility is an input.** Whether a maintenance generation can be absorbed
into a given primary request is reported by the snapshot; the decider does not
compute it.

## 5. The admission function

C.0 §9.2.1 governs the tiers, ageing, absorption and bounds; this section does
not restate them beyond what the decision needs.

`decide(&self, &ReadinessSnapshot) -> Option<AdmissionDecision>` walks the seven
tiers in order and returns the first that applies:

| Tier | Wins | Key condition |
| --- | --- | --- |
| 1 | Topology barrier | One is waiting |
| 2 | Unflip/recovery | Ready, and needed to restore a visible/correct desktop |
| 3 | Direct successor | Absorbs **every** aged maintenance identity that would otherwise win, **and** its unit holds the round-robin turn |
| 4 | Aged maintenance | Oldest ticket; stable `(CRTC, class)` tie-break |
| 5 | Homogeneous bundle | At least two CRTCs of the qualified group with ready generations, no barrier, all changed aged maintenance absorbed or serviced first |
| 6 | Primary | Oldest ready, round-robin per unit; the retirement successor is preferred when no other unit holds the turn |
| 7 | Non-aged maintenance | Oldest ticket |

**The decision carries**, so that it can be confirmed and tested unambiguously:

- the tier;
- the primary unit or units with their **exact generations**;
- the **absorbed** maintenance, each by identity and generation — only compatible
  generations that actually changed; an unchanged cursor is never carried
  (C.0 §9.2.1);
- the tickets it consumes;
- the maintenance that **ages** if it is confirmed: every ready, unsent identity
  that lost to an incompatible higher-priority admission.

**Symmetric absorption (tiers 4 and 7).** When a cursor or gamma identity wins and
the same CRTC has a compatible ready primary, the oldest such primary is
combined, without overtaking a barrier (C.0 §9.2.1).

**A successor that cannot absorb.** If a required cursor or gamma generation for
an affected CRTC is stale or incompatible, no plane-only successor is issued:
the successor stays in its slot (or is terminalized under its existing
direct/unflip rules) and the maintenance wins first (C.0 §9.2.1).

**Ageing.** Maintenance arriving while the device slot is occupied is aged on
arrival, keeping its original ticket. Barriers age the maintenance they overtake
(C.0 §9.2.1).

**Round-robin turn.** Each unit records the sequence number of its last primary
admission. Among units with a ready primary, the lowest sequence holds the turn,
so no unit takes two successive slots while another waits (C.0 §9.2.1).

*Mapping onto C.0, stated so review can challenge it:* §9.2.1 phrases the
round-robin "across CRTCs", while §9.1 defines intents per primary-plane
ownership unit. For a composed unit the two coincide. A grouped direct unit
spans several CRTCs and is **one** round-robin participant: its commit replaces
the whole output set at once, so treating it as several CRTCs would let one
logical owner collect several turns. This is a reading of C.0, not a change to
it; if review finds it wrong, the correction goes to C.0 as well.

**Tier 5 details (C.0 §9.2.1).** A CRTC not ready when the bundle is built is
not represented by carried state and earns no logical retirement; each distinct
included generation retires once from the same physical commit. With fewer than
two ready CRTCs, tier 6 applies: **there is no timer waiting for a bundle.**

**Bounds as invariants (C.0 §9.2.1):** an aged maintenance identity is admitted
after at most the commit in flight when it aged plus `N - 1` older-ticket
maintenance admissions; no unit takes two successive slots while another has a
ready primary. The decider keeps the per-class counts of intervening admissions,
and exceeding a bound is an **invariant failure**, not a statistic.

## 6. Two-phase confirmation

Admitting consumes state: tickets are spent, the round-robin turn advances,
losers age. The conductor may be unable to carry out a decision — the owner
refuses `begin`, qualification closed, the transport left `Owner`. Consuming
state before knowing would lose a ticket or skip a turn with nothing dispatched,
violating exactly the bounds section 5 measures.

So admission is **decide, then confirm**:

- `decide` is pure and changes nothing.
- The conductor attempts `begin`. On success it calls `commit(decision)`, which
  consumes the tickets, advances the turn and ages the losers. On failure it
  calls nothing, and the decider's state is untouched.
- `commit` verifies that every generation the decision named is still the
  current one before consuming anything (section 7 says what a mismatch means).

Rejected alternatives: mutating in `decide` and undoing on failure (an error path
that fails to undo corrupts fairness silently — the class of defect 2c-i kept
meeting); the decider calling the owner itself (breaks purity and ties the tests
to the owner).

## 7. The conductor

**Each wake, in this order:**

1. **On a retirement** (`CompletionRetired`): first enqueue the predecessor's
   completion and its deferred `Skip`s through 2c-i's protocol ledger. Nothing is
   published yet.
2. **If the device slot is free:** snapshot → `decide`. On a decision, build the
   request from its exact generations and 2c-i's resources and call
   `owner.begin`.
   - `begin` succeeds → `commit(decision)` and dispatch **in the same wake**
     (`DispatchTimingPolicy::ImmediateOnRetirement`, C.0 §9.2.1).
   - `begin` refuses → **nothing is confirmed**; record the reason; stop.
     **No retry**: the next real wake re-evaluates.
3. Only when the handler returns does the core publish the enqueued events. So
   admission and the kernel submission happen before wire publication, without
   breaking predecessor-before-`Skip` order (C.0 §9.2.1).

**At most one dispatch per wake:** there is one device slot, so after a
successful `begin` nothing else fits.

**A generation mismatch at `commit`.** The conductor runs on the core thread, so
nothing can change between `decide`, `begin` and `commit`. A mismatch is a
programming error and is treated as an **invariant failure that closes the
transport** through the existing failure path, consuming nothing half-way.

**Successor replacement.** When the decider returns a displaced generation, the
conductor runs it through 2c-i's never-submitted path — idle exactly once,
release pins, defer the `Skip` behind the predecessor (C.0 §9.1). The decider
touches no resource.

**Activation.** The conductor acts only with the device's transport in
**`Owner`**, reachable today only in fixtures carrying the writer-coverage
evidence of the 2c-i debt spec §4.4. In production the transport stays `Legacy`
and the conductor is inert (R8; stage 2c §6).

**Inputs in 2c-ii.** Producers are converted in 2c-iii, so intents are fed by
tests. Cursor and gamma use test payloads; no live maintenance payload is ever
synthesized from stale legacy state (stage 2c §4).

**`EBUSY`** is not a scheduling signal (C.0 §9.4). Seen with no live owner record,
the conductor does not retry, and C.0's failure path applies.

## 8. Out of scope, and the doors that must stay open

**The cursor coordinate lane** (C.0 §7.1, `OwnerMediatedLegacyMove`): a fast,
audited-cohort-only cursor move with its own per-plane `CoordinateSubmitting`
reservation that may overlap a primary commit. It has no ticket and does not
enter the tiers; stage 4 owns it. 2c-ii only has to know it exists and **never
absorb coordinate-only intent**. `SynchronousAtomicMove` — the cursor as an
ordinary atomic commit — is maintenance with a ticket and is in scope.

**Phase C.1 async** (C.0 §14): C.0 builds the generic primary successor slot and
C.1 inherits it — no second async queue. C.1 adds async capability/admission,
the singleton request shape, producer-fence transfer, `PAGE_FLIP_ASYNC` and its
completion contract, and requires a single active CRTC. 2c-ii implements nothing
async and adds no field for it in advance. Its one obligation is not to close the
door: the direct-successor slot is the one C.1 extends, and nothing in the
decider may assume a successor is synchronous in a way that prevents adding that
variant.

## 9. Relation to wlroots

Read from `~/Projects/wlroots` at `0.20.0-rc4-147-gbd75ebfe`, `backend/drm/drm.c`
and `types/output/output.c`.

**Where 2c-ii matches wlroots:**

- *Dispatch on retirement.* The `wlr_output` API requires a commit carrying a new
  buffer to wait for the frame event, emitted when the flip completes
  (`drm_connector_commit_state`); `ImmediateOnRetirement` is the same instant.
- *Cursor and gamma ride along.* `drm_connector_set_cursor`/`move_cursor` only
  stage state (`cursor_pending_fb`, `cursor_x/y`); it is folded into the output's
  next commit, and a buffer-less commit is made blocking to avoid `EBUSY`. Gamma
  is part of the output state. This is C.0's absorption, without tickets.
- *Multi-output commits.* One page flip may carry several connectors
  (`page_flip->connectors`, used by `wlr_backend_commit`), the analogue of tier 5.

**Where it diverges, deliberately — and C.0 made that choice, not 2c-ii.**
wlroots tracks `pending_page_flip` **per connector**: CRTCs commit independently
and concurrently, and a second non-blocking commit on a connector with a flip
pending simply fails ("a page-flip is already pending"). There is no
device-level queue, so there is nothing to arbitrate. C.0 §9 instead fixes **one
live atomic transaction per device** (`SingleSlotMultiCrtcCeiling`) to serialize
commits that could conflict through shared planes, connectors or routing, until
complete DRM-object conflict sets are tracked — "CRTC identity alone is
insufficient". **The seven tiers, round-robin and cross-CRTC fairness exist
because of that single slot**: when every CRTC shares one, something has to order
them and bound starvation.

With a single output, device slot and CRTC slot coincide, and the tiers only
order primary work against cursor and gamma; C.0 (absorption plus tickets) and
wlroots (cursor always on board) reach the same outcome by different means. C.0
names the way out — a future *Multi-CRTC Parallel Retirement* design — and the
pure decider keeps it cheap: it would change the slot model, not the callers.

## 10. Verification

### 10.1. What 2c-ii proves, and at what level

There is no production caller (R8): the conductor acts only with the transport
in `Owner`, in fixtures. **Everything 2c-ii proves is at fixture level, with the
real conductor and the real owner**, and every finding and status line says so.
No criterion promises evidence from a path that does not exist yet — the lesson
of the 2c-i debt spec §9.5, where two invariants rested on a route with no
production caller. That path appears when 2c-iii converts producers.

### 10.2. Exit criteria

Each has a named test and a named mutation that must break it. Mutations are
confirmed to have compiled.

| Criterion (source) | Mutation that must fail it |
| --- | --- |
| The seven tiers in order (stage 2c §2) | Swap two adjacent tiers |
| Supersession bounds: one slot per category (stage 2c §2) | Allow a second successor |
| No dispatch before readiness (stage 2c §2) | Admit a `Waiting` intent |
| Retirement ordering: predecessor, `Skip`, admission, publication (stage 2c §2, §4) | Publish before admitting |
| Two-phase: a refused `begin` consumes nothing (section 6) | Consume state inside `decide` |
| Round-robin: no unit takes two successive slots (C.0 §9.2.1) | Drop the last-admission record |
| Fairness under a **continuous direct-successor stream** (stage 2c §7) | Drop ageing on loss |
| Aged incompatible maintenance, symmetric absorption, **unchanged-cursor omission** (stage 2c §4) | Absorb an unchanged cursor |

The stage 2c design warns that **an empty maintenance queue is not evidence**, so
those cases run with real test cursor and gamma payloads. The starvation bounds
are asserted by the tests: exceeding one fails them.

### 10.3. Two plans

- **Plan A — primary:** slots and bounds, readiness, tiers 1, 2 and 6 (including
  the retirement successor's preference), round-robin, two-phase confirmation,
  and the conductor with retirement ordering.
- **Plan B — maintenance:** tickets and ageing, tiers 3, 4, 5 and 7, symmetric
  absorption, the homogeneous bundle, and the bounds measured under a continuous
  stream.

Each plan is reviewed by codex before implementation, and validated **task by
task with each task's full gate** (fmt, clippy, tests), not as one prototype.

### 10.4. Gate

`cargo +nightly fmt`; `cargo clippy --all-targets -- -D warnings` in the default
build and with `--features tcp-transport` and `--features xdmcp`; the
deterministic suites; `cargo check` for the Linux glibc, Linux musl and FreeBSD
targets. **And the full hardware gate**, which on this branch now means all three
of `render_acceptance`, `c0_2ci -- --ignored` and the library's other ignored
hardware tests (finding `2026-09-18-live-scene-fixture-regressions.md`) — run
only after asking the user, since this machine's GPU is also in personal use.

## 11. Questions for the plans

Stage 2c §7 treats its open questions as design work, not implementation
discretion. Of those that touch 2c-ii, the grouped-direct mapping is answered in
section 3; the guard-ownership and exclusive-route questions belong to 2c-i and
to stages 3/4. What is left below is placement and plumbing: each plan must name
its answer explicitly and the plan review checks it, but none of them can change
the decisions above.

- The conductor's module, and how it reaches the owner and 2c-i's state without a
  borrow cycle between platform, scene, drawable store and render engine. The
  constraint is fixed here: the decider borrows nothing, and the conductor
  holds no long-lived borrow across a wake.
- How the snapshot is assembled from 2c-i's state — role vacancy, pool buffer
  phase, producer waits — and where each value is read.
- Whether the census tool (`tools/guard-census.py`) is extended to the decider's
  refusal points (`return None` guards), or its tier conditions are covered by the
  named mutations of section 10.2 alone.
- How the `HomogeneousCompletionGroup` membership reaches the snapshot; qualifying
  the group on hardware remains stage 3 and C.0 §16.3.
