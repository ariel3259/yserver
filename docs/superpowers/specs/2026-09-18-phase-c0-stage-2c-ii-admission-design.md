# Phase C.0 stage 2c-ii — bounded intents and admission

**Status:** design, revision 2 (2026-09-18). Its five design sections (units,
readiness, the decision, the conductor, verification — here sections 3–7 and 10)
were approved one by one with the user in brainstorming, and the wlroots
comparison (section 9) was requested there. Revision 2 incorporates codex round 1
(`../findings/2026-09-18-stage-2c-ii-design-review-round1.md`: 3 blocking, 1
major, all four verified against the tree and accepted): fairness per CRTC
(B-1), the confirmation boundary moved to the send (B-2), tier 5 takes every
ready CRTC (B-3), barriers counted apart from the bound and ticket-lifecycle
mutations (M-1). One question C.0 does not answer is open in section 11.1.
Two implementation plans follow (section 10.3).

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

**Carried items.** The 2c-i F-15 review listed four things 2c-ii's spec must
carry. Their disposition:

1. *The per-guard-clause pass over `resources/{mod,commit,gpu,transport}.rs`* —
   **done**, by the 2c-i debt stage inserted before this one: 71 census sites,
   39 proven by their own test, 32 caught by others, zero survivors (finding
   `2026-09-17-stage-2c-i-debt-census-session-2.md`). 2c-ii's own guards follow
   the same rule through section 10.2's named mutations.
2. *F13c-m1* and 3. *F13c-m2* — **closed**: session 2 of the debt stage bound
   pool-husk accounting to an identity-bearing registration that fails closed
   when dropped or skipped (`a87601c2`).
4. *F13b-D1* — the dispatched `CommitResources` carries no present-pin leases
   by value. **Stays with 2c-iii**, as the F-13b review decided: adopting leases
   by value is the activation half, and the leases come from the real producer,
   which 2c-iii converts. 2c-ii must not make it worse: the conductor's request
   builder **takes the primary's leases as an input** and moves them into the
   request, so 2c-iii supplies real ones without changing the conductor. A
   builder that constructs an empty lease set on its own is out of bounds.

## 2. Architecture

Two parts, with a strict boundary.

- **The decider** — a new module in `kms/owner`. Pure: it holds only
  *descriptors* of intents (generations, readiness, tickets, turns), owns no
  resource and performs no I/O. Given a readiness snapshot it returns an
  `AdmissionDecision`.
- **The conductor** — one `AdmissionConductor` per device, on the platform side.
  The only component that talks to all three of: the decider, the
  `DeviceCommitOwner` (`begin`/`send_on`), and 2c-i's resources (to build the
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

Units are **storage and resource containers**. Round-robin and fairness are
accounted **per CRTC**, as C.0 §9.2.1 states (section 5). This answers the
fourth question stage 2c §7 left open — how grouped direct frames map to
per-device admission: one grouped unit is admitted as a whole and counts as a
service for every CRTC it covers, while per-output evidence and the shared
source's release stay with 2c-i's ledger, which does not release a shared source
at the first output.

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
| 3 | Direct successor | Absorbs **every** aged maintenance identity that would otherwise win, **and** the round-robin rule permits every CRTC it covers |
| 4 | Aged maintenance | Oldest ticket; stable `(CRTC, class)` tie-break |
| 5 | Homogeneous bundle | At least two CRTCs of the qualified group ready, no barrier, all changed aged maintenance absorbed or serviced first; the bundle includes **every** ready CRTC of the group |
| 6 | Primary | Oldest ready, round-robin per CRTC; the retirement successor is preferred when no different CRTC is owed the turn |
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

**Round-robin turn — per CRTC (C.0 §9.2.1).** Each CRTC records the sequence
number of the last admission that carried a primary for it. A primary admission
that covers several CRTCs (a grouped direct unit, a tier-5 bundle) marks
**every** CRTC it includes as served. A candidate is eligible for tiers 3 and 6
only if **none** of the CRTCs it covers would take a second successive slot while
another CRTC has a ready primary and is owed service. So neither a composed
intent on a CRTC just served by a grouped commit, nor a grouped commit right
after one of its CRTCs was served alone, can jump the queue.

Revision 1 counted turns per unit instead. The round-1 review (B-1) showed that
this is weaker than C.0: after a grouped `AB` commit, a composed `A` could win the
next slot while `B` waited, with no *unit* taking two turns. Units remain only
the storage of section 3.

**Tier 5 details (C.0 §9.2.1).** The bundle takes the oldest ready synchronous
generation of **every** ready CRTC in the `HomogeneousCompletionGroup`, each
with canonical completion coverage — never a subset of the ready ones, which a
continuous two-CRTC stream could otherwise use to starve a third. A CRTC not
ready when the bundle is built is not represented by carried state and earns no
logical retirement; each distinct included generation retires once from the same
physical commit. With fewer than two ready CRTCs, tier 6 applies: **there is no
timer waiting for a bundle.**

**Bounds as invariants (C.0 §9.2.1):**

- With `N` incompatible aged maintenance identities, each is admitted after at
  most the one commit already submitted when it aged, `N - 1` older-ticket
  maintenance admissions, and owner dispatch latency.
- **Finite topology/unflip/recovery barriers may interrupt that bound.** They are
  counted separately and are not a violation; what they may never do is reset or
  reorder a surviving ticket.
- No CRTC takes two successive slots while another CRTC has a ready primary.

The decider counts intervening admissions per class, with barrier admissions in
their own count. Exceeding a bound **after discounting the barriers** is an
**invariant failure**, not a statistic.

## 6. Two-phase confirmation

Admitting consumes state: tickets are spent, the round-robin turn advances,
losers age. The conductor may be unable to carry out a decision, and not only at
`begin`: `DeviceCommitOwner::begin_with_context` installs a live record, reserves
the slot and takes the ledger, but the later `send_on` can still refuse **before
any IPC** (`Reaped`, `Stalled`, `AlreadyInFlight`, `ReservationMismatch`,
`BoundaryViolation`, `TransportGateRefused`), terminalizing the record as
`NeverDispatched`. Consuming state before the send boundary would lose a ticket
or skip a turn with nothing dispatched, violating exactly the bounds section 5
measures.

The confirmation boundary is therefore **the send boundary, not `begin`**:

1. `decide` is pure and changes nothing.
2. `lock(decision) -> AdmissionToken` checks that every generation the decision
   names is still current and marks one admission as pending. It consumes no
   fairness state. While a token exists the decider refuses another `lock`. This
   is where a generation mismatch is detected — **before** anything is handed to
   the owner.
3. The conductor calls `begin`, then `send_on`.
4. The token is consumed exactly once, by value:
   - **`confirm(token)`** when the owner reports `Dispatched` — `send` returned
     `Ok`, or `SendError::Ipc`, which means the write was attempted and the owner
     already treats the record as dispatched. Only now are the tickets consumed,
     the turn advanced and the losers aged.
   - **`abort(token)`** when `begin` refuses, or when `send_on` refuses before
     IPC. Nothing is consumed; the decider is exactly as before `lock`.

Rejected alternatives: mutating in `decide` and undoing on failure (an error path
that fails to undo corrupts fairness silently — the class of defect 2c-i kept
meeting); confirming at `begin` (revision 1 — the round-1 review, B-2, showed it
loses a ticket on a pre-IPC send refusal); the decider calling the owner itself
(breaks purity and ties the tests to the owner).

## 7. The conductor

**Each wake, in this order:**

1. **On a retirement** (`CompletionRetired`): first enqueue the predecessor's
   completion and its deferred `Skip`s through 2c-i's protocol ledger. Nothing is
   published yet.
2. **If the device slot is free:** snapshot → `decide` → `lock`. Build the
   request from the decision's exact generations and 2c-i's resources, then
   `begin` and `send_on` **in the same wake**
   (`DispatchTimingPolicy::ImmediateOnRetirement`, C.0 §9.2.1).
   - Dispatched → `confirm(token)`.
   - `begin` refuses → `abort(token)`; the owner took nothing.
   - `send_on` refuses before IPC → `abort(token)`. The owner has already retired
     the record as `NeverDispatched` and returned its ledger in the refusal's
     events (`Terminal`, `ResourcesReleased`, `ResourcesStillCurrent`); the
     conductor hands those to 2c-i's ledger, which disposes of each resource on
     its never-dispatched path. Composed generations and maintenance desired
     state are desired state and stay in the decider. A direct successor whose
     leases that path terminalized is **withdrawn** from its slot — a withdrawal
     consumes no ticket and advances no turn.
   - In every refusal: record the reason, stop. **No retry**: the next real wake
     re-evaluates.
3. Only when the handler returns does the core publish the enqueued events. So
   admission and the kernel submission happen before wire publication, without
   breaking predecessor-before-`Skip` order (C.0 §9.2.1).

**At most one dispatch per wake:** there is one device slot, so after a
successful `begin` nothing else fits.

**A generation mismatch at `lock`.** The conductor runs on the core thread, so
nothing can change between `decide` and `lock`. A mismatch is a programming
error and is treated as an **invariant failure that closes the transport**
through the existing failure path. Because `lock` precedes `begin`, the owner
holds nothing at that point and no `cancel_live` is needed. Between `lock` and
`confirm` only the conductor runs, and it changes no descriptor, so `confirm`
cannot see a mismatch.

**After dispatch**, the commit follows the owner's normal lifecycle (C.0 §10) and
2c-i's ledger. Its admission is already confirmed: the slot really was
occupied.

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
| Two-phase: a refused `begin` **and each pre-IPC `send_on` refusal** (`Reaped`, `Stalled`, `AlreadyInFlight`, `ReservationMismatch`, `BoundaryViolation`, `TransportGateRefused`) consume nothing; `Ok` and `SendError::Ipc` confirm (section 6) | Confirm at `begin` instead of at the send; consume state inside `decide` |
| A generation mismatch is caught at `lock`, before the owner holds anything (section 7) | Move the check after `begin` |
| Round-robin per CRTC: no CRTC takes two successive slots, including grouped→composed and composed→grouped transitions (C.0 §9.2.1) | Mark only one CRTC of a multi-CRTC admission as served; drop the last-admission record |
| Tier 5 includes **every** ready CRTC of the group, tested with three or more CRTCs (C.0 §9.2.1) | Drop one ready member from the bundle |
| A ticket survives payload replacement (C.0 §9.2.1) | Reset the ticket on replacement |
| An update arriving while its identity is submitted gets a **new** ticket (C.0 §9.2.1) | Reuse the consumed ticket |
| A barrier interrupts the bound without resetting or reordering surviving tickets, and is counted apart (C.0 §9.2.1) | Reset relative age at a barrier; count barrier admissions against the maintenance bound |
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

### 11.1. Open design question — must be settled before plan B

**What happens to absorbed maintenance when a dispatched commit is rejected
by the kernel?** Section 6 confirms at the send boundary, so the tickets are
spent. C.0 §9.2 says a failure leaves the prior current state authoritative, but
nowhere says whether a cursor or gamma generation carried by the rejected commit
re-enters admission with its original ticket (aged), gets a new one, or is
dropped as incompatible. The choice affects the starvation bound: a new ticket
could push it back indefinitely, while re-entering with the old ticket could
loop on a payload the kernel will always reject. This is a gap in C.0, not
plumbing; it is recorded here per the authority rule and answered in C.0 (or by
the user) before plan B, which owns tickets. Plan A (primary only) does not
depend on it.
