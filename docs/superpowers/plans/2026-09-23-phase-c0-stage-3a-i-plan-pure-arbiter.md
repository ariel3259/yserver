# Stage 3a-i — the pure lifecycle arbiter

> **Implementer:** codex (model `gpt-6-luna`, reasoning effort `xhigh`), run **without sandbox** (`--sandbox danger-full-access`, user-authorized) with `< /dev/null`. Hard rules, restated in every prompt: **no git write commands** (the coordinator verifies and commits); this plan needs **no GPU and no `#[ignore]` test** — run none of them, never `_drm`, never `render_acceptance`, never an unfiltered `--ignored`; no deletes outside the worktree; remove temporary instrumentation before finishing. **You write the implementation and the tests**; this plan gives the interfaces, the invariants, the named tests with the scenario each must exercise, and the mutations each must catch. Execute tasks in order, one at a time; stop with the tree dirty after each task. **Do not ask for approval inside a run** — if the plan leaves a real design choice open, or something it states does not hold in the code or in C.0, stop and report it (F8); never silently substitute a test shape.

**Revision 7 (2026-09-23)** — Task 3 F8 from the implementer (stopped
before editing): C.0's `REC-6` table covers only an **active** incident, and a
boundary row of Table U has no "without incident" case. F-4 defines the fate
after `RecoveryFailed` from C.0's own §6.4 row for that state; U-4 makes a
boundary row's fresh id a mandatory input.

**Revision 6 (2026-09-23)** — codex round 5
(`../findings/2026-09-23-stage-3a-i-plan-review-round5.md`: 1 blocking; no
regression): an incident's representative event is pending while the incident
lives and ends exactly once when it resolves (U-1b), with
`Invalidated(RecoveryFailed)` as a stated extension of C.0's reason list.

**Revision 5 (2026-09-23)** — codex round 4
(`../findings/2026-09-23-stage-3a-i-plan-review-round4.md`: 1 blocking,
1 major; every round-3 finding applied): the reported loss event of a
teardown row with no incident ends `Invalidated` by that row (B-1); a
coordinator → arbiter → Table U handoff test covers every active row (M-1).

**Revision 4 (2026-09-23)** — codex round 3
(`../findings/2026-09-23-stage-3a-i-plan-review-round3.md`: 3 blocking, 1
major, all verified): a receipt proves only the transfer its own transition
requested — a new winner requests and awaits its own (B-1); the row active
when a loss is observed decides, with no "ordinary work" exception (B-2); an
incident created during a DPMS transition is paused only if the device's
target is off (B-3); a same-identity rebuild with no incident follows the
normal-live row (M-1).

**Revision 3 (2026-09-23)** — codex round 2
(`../findings/2026-09-23-stage-3a-i-plan-review-round2.md`: 4 blocking, 1
major, all verified against C.0) showed revision 2's R4-2a and R6-0 traded.
**Task 3 is rewritten** around C.0's two normative recovery tables — §10's
"encountered `CompletionUnknown`" table by active row (Table U) and `REC-6`
(Table F) — so a loss during `VTRelease`, removal or shutdown creates no
incident (B-2), the first incident's representative is a coordinator event id
(B-3), and a logical DPMS-on resumes a paused incident (B-4); **R4-2a is
rewritten** as "logical now, physical after the receipts", with receipts
attributed to the current winner (B-1); the mixed-arrival test covers every
active kind (M-1).

**Revision 2 (2026-09-23)** — incorporates codex round 1
(`../findings/2026-09-23-stage-3a-i-plan-review-round1.md`: 2 blocking, 4
major, 1 minor, coverage complete; all verified against C.0 and accepted).
**B-1** supersession advances the winner only after the safety actions are
acknowledged (R4-2a); **B-2** the first completion loss creates the incident
(R6-0); **M-1** the `REC-6` matrix is now Task 3 and the arbiter Task 4, so the
arbiter is built on the final matrix; **M-2** a mixed-arrival ledger test;
**M-3** ordinary work's tag has no transition id, with evidence; **M-4** DPMS
while `Poisoned` stays `Deferred(ReadinessClosed)`, never `Applied`; **m-1** a
replaced `Deferred` representative ends as `SupersededBy`.

**Revision 1 (2026-09-23, coordinator).**

**Goal:** The lifecycle decision layer of stage 3, as **pure code**: its
identities, the per-device `LifecycleDesired` snapshot with `REC-5`
coalescing and dispositions, the per-device arbiter with `REC-4` precedence,
supersession and epoch rules, the `REC-6` recovery-fate matrix, and the global
coordinator with the DPMS projection. No I/O, no fd, no executor call, no
resource, no backend field. Plan 3a-ii wires it to production (the driver,
`Tier::Topology`, DPMS execution); until then these types have **no
production caller**, which is deliberate and stated (stage 3a design §3.2):
their claims are about the decision layer itself and are proven by exhaustive
tables, not by a route.

**Why split:** the stage 3a design is about fourteen tasks. Measured in this
project, plan size drives defect density (14 tasks → 2 blocking review
findings; 21 → 24; 23 → 26). 3a-i is the part with no integration surface;
3a-ii is written after 3a-i is accepted.

**Authority:**
- C.0 `docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md`
  §6.4 (the table, `REC-1`..`REC-6`, lines 754–935) and §16.2 items 57, 63,
  64, 65, 66, 67 — every row of those is an exit criterion here.
- Stage 3 umbrella `docs/superpowers/specs/2026-09-22-phase-c0-stage-3-lifecycle-design.md`
  revision 5, §2.1–2.3.
- Stage 3a design `docs/superpowers/specs/2026-09-23-phase-c0-stage-3a-arbiter-and-dpms-design.md`
  revision 6 (user-approved decisions), §3.1, §3.2, §3.3 (coordinator part),
  §3.6 (the result table, as arbiter inputs), §3.7 (failure edges, as arbiter
  inputs).

Read all three before Task 1.

## Design decisions this plan fixes

1. **Where.** The layer lives under `crates/yserver/src/kms/owner/lifecycle/`:
   today's `owner/lifecycle.rs` becomes `lifecycle/ids.rs` unchanged (its
   tests move with it), beside new modules for the desired snapshot, the
   arbiter, the recovery matrix and the coordinator. The names inside are
   yours; the public surface is what the tasks below state.
2. **Pure means pure.** No module here imports anything from `kms::render`,
   `kms::executor`, `drm` or `platform`, and nothing here holds a device key
   type from those modules: a device is identified by an opaque key type the
   caller supplies (generic parameter or a small newtype of this layer). A
   test (`c0_3a_layer_imports_nothing_effectful`) reads the module sources and
   fails on any such import.
3. **Inputs and outputs are values.** The arbiter is a state machine:
   `apply(input) -> Vec<Action>`. Inputs are projected events and acknowledged
   outcomes; actions are typed values the 3a-ii driver will execute. The
   arbiter never assumes an action succeeded: success comes back as an input.
4. **C.0's names verbatim** for kinds, states and dispositions (user-approved).
5. **Tests are named `c0_3a_`**, ordinary `#[test]`s (no `#[ignore]`), and
   tables are **generated over the enums** (iterate every variant), never
   hand-listed, so a new variant cannot silently escape a table. Each enum
   used in a table exposes its full variant list for that purpose.

## Global constraints

- Production is byte-for-byte unchanged: nothing outside
  `owner/lifecycle/` changes except the `mod` line and the path of the moved
  identities.
- Every identity counter is checked and never wraps (the existing
  `LifecycleEpochId` pattern).
- Bounded state: the arbiter's memory is bounded by the number of fields and
  the current protocol-output domain, never by event count (C.0 `REC-5`).

## Task 1 — identities and value types

**Deliver:** `LifecycleEventId` (monotonic, checked, allocated only by the
coordinator); `LifecycleKind` — the ten kinds with a total order equal to
C.0's precedence (`Shutdown` highest … `NormalRecovery` lowest);
`Disposition` — `Applied(transition)`, `AbsorbedByEvent(event)`,
`AbsorbedByTransition(transition)`, `Invalidated(reason)`,
`SupersededBy(event)` (terminal) and `Deferred(prerequisite)` (nonterminal);
`InvalidationReason` — exactly C.0's (shutdown, device removed, VT release,
protocol-output removal, newer generation) plus the **four** authorized
external boundaries at which `REC-6` invalidates an existing incident, one
reason per winning kind — `DeviceAddedOrReplaced`, `VTAcquire`,
`AdministrativeReprobe`, `IdentityChangingHotplug` (C.0: "terminally
invalidated by that authorized external boundary"; Task 1 F8, 2026-09-23);
plus `RecoveryFailed` — **an explicit extension of C.0's reason list**, for
the representative of an incident whose sole attempt failed (U-1b), since none
of C.0's listed reasons describes that outcome; `Prerequisite` — seat released,
device absent, and the two stage 3a failure prerequisites
`TopologyLatched(generation)` and `ReadinessClosed`; `DeviceLifecycleState`
— the nine §6.4 states, `Recovering` carrying its `RecoveryId`; `RecoveryId`
— its own counter, with **no** conversion from or to `LifecycleTransitionId`;
`WorkTag` — incarnation, `LifecycleEpochId`, and an **optional**
`LifecycleTransitionId`: `Some` for transition-owned work, `None` for ordinary
`Ready` work (C.0 item 65; the existing owner record already carries an
optional transition id) — the incarnation as an opaque value supplied by the
caller, decision 2. `TransitionTag` is the `Some` case.

**Tests:**

| Test | Scenario | Must fail under |
| --- | --- | --- |
| `c0_3a_kind_order_is_c0_precedence` | the order over every variant equals C.0's list, in full | **T1** swap two adjacent kinds |
| `c0_3a_event_id_is_checked` | first/next/`u64::MAX` has no next | **T2** make the increment wrapping |
| `c0_3a_terminal_and_deferred_are_distinct` | every `Disposition` variant classifies as terminal or not, and only `Deferred` is not | **T3** classify `Deferred` as terminal |
| `c0_3a_layer_imports_nothing_effectful` | decision 2's source check | **T4** add a `use crate::kms::render` line |

Plus a compile-fail test (`crates/yserver/tests/compile_fail/`, the existing
harness) that converting a `LifecycleTransitionId` into a `RecoveryId` does
not compile (C.0: a recovery id is "never derived from
`LifecycleTransitionId`").

## Task 2 — `LifecycleDesired` and `REC-5`

**Deliver:** the per-device snapshot (presence + identity epoch, seat target +
epoch as projected by the coordinator, administrative reprobe epoch,
`topology_dirty` + discovery epoch + change class, `dpms_target` per stable
protocol output + the projected DPMS epoch, the recovery incident) and the
disposition ledger for the device's representatives.

**Invariants (C.0 `REC-5`):**
- **R5-1** At most one representative `LifecycleEventId` per field; the
  projected target map is bounded by the current output domain.
- **R5-2** Equal-kind coalescing, typed exactly as C.0 states: shutdown and
  removal duplicates → `AbsorbedByEvent(representative)`; newer presence, seat,
  reprobe and DPMS generations replace the representative and give the
  displaced one `SupersededBy(newer)`; topology events keep only the newest
  discovery epoch and supersede the displaced representative (execution
  rediscovers, never replays a payload); duplicate normal-recovery events →
  `AbsorbedByEvent` of the existing incident's representative, never a new
  incident.
- **R5-3** Every event id reaches **exactly one** terminal disposition, which
  is then immutable; `Deferred` is the only nonterminal one and must later
  become `Applied`, an `AbsorbedBy*`, `Invalidated`, or — when a newer
  generation of the same field replaces it — `SupersededBy(newer)` (C.0's
  typed latest-wins rule, which applies to a deferred representative too).
- **R5-4** A DPMS projection replaces every current per-output target in the
  same epoch; a newly discovered output inherits the current global level
  before any installation (umbrella rev-2 M-2); removing an output invalidates
  **only** its projection, exactly once.

**Tests:**

| Test | Scenario | Must fail under |
| --- | --- | --- |
| `c0_3a_equal_kind_coalescing_table` | for **every** kind, two events of that kind: the representative and the displaced id's disposition match R5-2 (C.0 item 64) | **T5** make DPMS coalesce by `AbsorbedByEvent` instead of `SupersededBy`; **T6** keep the older topology epoch |
| `c0_3a_storm_stays_bounded` | 10 000 events of mixed kinds: retained representatives never exceed the field count, and every displaced id is terminal | **T7** retain displaced representatives in a list |
| `c0_3a_terminal_disposition_is_immutable` | every terminal disposition, then an attempt to assign another: refused, the first stays | **T8** allow overwriting a terminal disposition |
| `c0_3a_deferred_reaches_one_terminal` | a `Deferred(SeatReleased)` target, then seat acquired / shutdown / newer generation: each path ends in exactly one terminal disposition, asserted **exactly** — `Applied`, `Invalidated(Shutdown)`, `SupersededBy(newer)` respectively | **T9** leave the representative `Deferred` after the prerequisite returns |
| `c0_3a_projection_follows_the_output_domain` | global off, then an output is added (inherits off before install), then another is removed (only its projection invalidated, once) (item 66) | **T10** add the new output with target on; **T11** invalidate every projection on one removal |

## Task 3 — the two recovery tables and `REC-1` accounting *(rewritten in revision 3)*

**Why rewritten:** revision 2 treated every completion loss alike. C.0 has
**two** normative tables and this task implements both, as pure functions,
before the arbiter uses them (round-2 B-2, B-3, B-4):
- **Table U** — C.0 §10, "The transition that encountered `CompletionUnknown`
  continues according to this table" (lines 2044–2055): what happens when a
  completion loss is encountered, **by the row active at that moment**.
- **Table F** — C.0 `REC-6` (lines 918–935): what happens to an **existing**
  incident when a lifecycle kind wins.

**Deliver:** the incident record (`RecoveryId`, its one-attempt budget, its
state: active, paused, attempting, `RecoveryFailed`); the per-device
`RecoveryId` allocator (this task owns it; Task 4's arbiter never allocates
one); Table U and Table F as functions over (row or winning kind, incident
state) returning a typed outcome.

**Invariants:**
- **U-1** *(round-2 B-2)* Table U by row, exactly: **normal live operation** (no lifecycle transition active when the loss is **observed**; the active row decides even for a loss of ordinary work, round-3 B-2) → stop admission, logical
  withdrawal of protocol work, quarantine both states, and create the incident
  (U-3) whose sole attempt follows reap; **`VTRelease`** active → release the
  seat logically **now**, record the `REC-6` invalidation of any existing
  incident, create **no** incident, keep the unknown commit in quarantine;
  **`DeviceRemoved`** active → `Invalidated(DeviceRemoved)` for any incident,
  logical withdrawal now, no incident; **`Shutdown`** active →
  `Invalidated(Shutdown)`, no incident; **same-identity `TopologyRebuild`**
  active → the existing incident and quarantine transfer to the rebuild, whose
  qualified install consumes it or whose failure terminalizes that same id as
  `RecoveryFailed`; **if none exists** (round-3 M-1), the loss is handled as a
  normal-live loss — the incident is created by U-3 with the coordinator's
  event id, its one attempt runs after reap — and the rebuild is terminalized,
  never given an id of its own;
  **`DeviceAddedOrReplaced`/`VTAcquire`/`AdministrativeReprobe`/
  `IdentityChangingHotplug`** active → their fresh id (at most one) reaches
  `RecoveryFailed` on failure or unknown; **`DPMS`** active → the device is poisoned, no KMS mutation follows, and the
  incident is created (U-3); it is **paused only if the device's current DPMS
  target is off** (Table F's DPMS row defers it "on `dpms_target = On`"); with
  the target on it is active and its attempt proceeds after reap
  (round-3 B-3).
- **U-1a** *(rev 5, round-4 B-1)* **The reported event always ends.** The
  loss's own event id (U-3) reaches exactly one terminal disposition in every
  row: normal live → it is the new incident's representative; `DPMS` and a
  rebuild with no incident → the same; an existing incident →
  `AbsorbedByEvent(its representative)`; `VTRelease`, `DeviceRemoved`,
  `Shutdown` with **no** incident → `Invalidated(VTRelease)`,
  `Invalidated(DeviceRemoved)`, `Invalidated(Shutdown)` respectively, recorded
  before the row completes, and no recovery intent is left for later
  convergence (a later `VTAcquire` allocates a fresh id only under Table F).
- **U-1b** *(rev 6, round-5 B-1)* **A representative ends when its incident
  does.** Being an incident's representative is an identity, not a
  disposition: while the incident lives (active, paused, attempting, or
  transferred to a rebuild) its representative is pending, and it reaches
  exactly one terminal disposition when the incident resolves — the recovery
  installs and qualifies → `Applied(the installing transition)`; the sole
  attempt fails or is unknown → `Invalidated(RecoveryFailed)`; a Table F row
  invalidates the incident → `Invalidated(that row's reason)`; a Table F
  transfer keeps it pending under the same id. Events absorbed by it are
  terminal at once (`AbsorbedByEvent(representative)`).
- **U-4** *(rev 7, Task 3 F8)* **A boundary row always holds its fresh id.**
  `REC-6` (line 926): the boundary "allocates exactly one fresh `RecoveryId`
  before its first attempt". A completion loss during a
  `DeviceAddedOrReplaced`, `VTAcquire`, `AdministrativeReprobe` or
  `IdentityChangingHotplug` attempt therefore always has that id: the Table U
  row for those kinds takes it as a **mandatory input** (the type makes the
  no-id case unrepresentable) and returns `RecoveryFailed` for it. Their
  "without incident" variant is not tested because it cannot be constructed.
- **U-2** Every row's outcome says separately what is **logical** (immediate:
  seat release, withdrawal, protocol terminalization) and what is **physical**
  (waits: reap, fd-family close, fresh install). Task 4 uses that split.
- **U-3** *(round-2 B-3)* **The first incident has an event id.** A completion
  loss is observed by the driver, which reports it to the **coordinator** as a
  `NormalRecovery` event; the coordinator allocates its `LifecycleEventId`
  like any other event, and the creation input carries that id. The incident's
  representative is that id. A later loss or normal-recovery event while the
  incident exists is `AbsorbedByEvent(representative)` and allocates no
  `RecoveryId`.
- **F-1** Table F exactly: `Shutdown`, `DeviceRemoved`, `VTRelease` →
  `Invalidated` with that reason; `DeviceAddedOrReplaced`, `VTAcquire`,
  `AdministrativeReprobe`, `IdentityChangingHotplug` → the old incident
  invalidated by that boundary and at most one fresh `RecoveryId` if recovery
  is still required; same-identity `TopologyRebuild` → the same incident and
  remaining budget transferred; `DPMS` → paused, never consumed, cloned or
  revived; `NormalRecovery` → continued, equal events absorbed.
- **F-2** *(round-2 B-4)* **The DPMS resume boundary is logical.** A DPMS-off
  pauses the incident and defers it on `dpms_target = On`; a DPMS-on **resumes
  it when the DPMS request is logically applied** — on a poisoned device that
  is the logical power change itself, since no KMS mutation happens (C.0 §10
  DPMS row: "DPMS-on resumes an existing paused attempt"). The resumed attempt
  is the same id with the same remaining budget; DPMS never creates an id and
  never revives `RecoveryFailed`.
- **F-3** No path both invalidates and transfers an id, allocates two ids, or
  spends an attempt twice (item 67).
- **F-4** *(rev 7, Task 3 F8)* **After `RecoveryFailed`.** `REC-6` defines
  outcomes for an active incident only. A failed incident is terminal (its
  representative already `Invalidated(RecoveryFailed)`, U-1b) and the device
  stays `RecoveryFailed`. C.0's §6.4 row for that state (line 763) names its
  only exits: "actual hotplug identity change, VT reacquire, administrative
  reprobe, or restart creates at most one fresh attempt" — together with
  `DeviceAddedOrReplaced` for a newly present identity (`REC-6`, line 924).
  So, for a device in `RecoveryFailed`: `DeviceAddedOrReplaced`, `VTAcquire`,
  `AdministrativeReprobe`, `IdentityChangingHotplug` → if installation is
  required, allocate **one** fresh `RecoveryId` owned by that boundary
  transition; `Shutdown`, `DeviceRemoved`, `VTRelease`, same-identity
  `TopologyRebuild`, `DPMS` → the failed record is kept, nothing is allocated,
  the device stays `RecoveryFailed`; a `NormalRecovery` event (a later loss
  report) → `Invalidated(RecoveryFailed)`, no attempt (`REC-1`).
  **A boundary's fresh id has no representative event**: its outcome is an
  incident state (consumed as recovered by a qualified install, or
  `RecoveryFailed` on failure or unknown), recorded on the boundary
  transition, while the boundary's own event keeps its `REC-5` disposition
  for its own field. U-1b's representative rule applies only to incidents
  created from a loss report (U-3).
- **R1** `REC-1`: one automatic attempt per incident; any failure or unknown
  during it → `RecoveryFailed`; timers, DPMS, client traffic and queued intents
  cannot create another attempt.

**Tests:**

| Test | Scenario | Must fail under |
| --- | --- | --- |
| `c0_3a_unknown_table_by_row` | for **every** row of Table U, with and without an existing incident where both are constructible (the four boundary rows take their mandatory fresh id, U-4): the exact logical outcome, physical outcome, incident created or not, and the `REC-6` fate recorded | **T38** create an incident on a loss encountered by `VTRelease`; **T39** poison-and-create during `Shutdown` |
| `c0_3a_recovery_matrix_is_total` | every winning kind × an active incident: the exact outcome of F-1 (item 67); and every winning kind × a device in `RecoveryFailed`: the exact outcome of F-4 — a fresh id only for the four boundaries, never for the others, and a later loss report `Invalidated(RecoveryFailed)` | **T23** transfer the incident on `IdentityChangingHotplug`; **T24** let DPMS allocate a fresh id; **T51** let a same-identity `TopologyRebuild` allocate a fresh id after `RecoveryFailed`; **T52** give a boundary's fresh id a representative event |
| `c0_3a_no_double_fate` | every row of both tables: never both invalidated and transferred, at most one id allocated | **T25** allocate a fresh id and also transfer the old one on `VTAcquire` |
| `c0_3a_first_loss_creates_one_incident` | no incident; a loss during normal live operation reported with a coordinator-allocated event id E: exactly one `RecoveryId`, representative E; then a second loss (id E2) and three normal-recovery events: same `RecoveryId`, each `AbsorbedByEvent(E)` exactly; E stays pending while the incident lives, then ends `Applied(installing transition)` on a successful attempt and — in a second run — `Invalidated(RecoveryFailed)` on a failed one (U-1b) | **T31** allocate a new id on the second loss; **T40** create the incident with no representative; **T50** leave E without a disposition after `RecoveryFailed` |
| `c0_3a_unknown_during_teardown_follows_the_active_row` | an ordinary commit becomes unknown while `VTRelease`, `DeviceRemoved` and `Shutdown` are each active: each follows its own row — no incident, logical obligations at once, and the reported event ends exactly `Invalidated(VTRelease)`, `Invalidated(DeviceRemoved)`, `Invalidated(Shutdown)` before the row completes (U-1a) | **T45** treat an ordinary-work loss as normal live operation during `VTRelease`; **T48** leave the reported event without a disposition when no incident exists |
| `c0_3a_first_loss_during_dpms_on_is_not_paused` | no incident; a DPMS-on commit becomes unknown: the incident is created, **active** (target on), and its attempt is authorized after reap; the same during DPMS-off: created **paused** | **T46** pause every incident created during a DPMS transition |
| `c0_3a_first_loss_during_rebuild_is_normal_live` | no incident; a same-identity rebuild's commit becomes unknown: one incident via the coordinator's event id, attempt after reap, rebuild terminalized with no id of its own | **T47** give the rebuild a `RecoveryId` of its own |
| `c0_3a_dpms_on_resumes_the_paused_incident` | an incident, DPMS off (paused), the device poisoned, DPMS on applied logically: the same id resumes with the same remaining budget, no id allocated, no KMS action, the DPMS representative still `Deferred(ReadinessClosed)` | **T41** keep the incident paused until a KMS installation; **T42** reset the attempt budget on resume |
| `c0_3a_one_attempt_per_incident` | an attempt fails, then DPMS on, a timer, and repeated normal-recovery events: no second attempt, state `RecoveryFailed` | **T26** let DPMS-on resume a `RecoveryFailed` incident |

## Task 4 — the arbiter: `REC-4` precedence, supersession, convergence, epoch

**Deliver:** the per-device arbiter: the §6.4 state, at most one
`LifecycleTransition { id, kind, phase }`, the transition-id allocator (the
arbiter publishes ids; `LifecycleTransitionId` keeps no public `next`), and
the lifecycle epoch.

**Invariants:**
- **R4-1** At most one transition exists, ever.
- **R4-2** A higher-precedence event supersedes the active transition: the
  actions close admission and alias creation, cancel pre-submit work, request
  each Present's terminalization once, and transfer quarantine to the winner.
  A transition whose commit is `Submitting` or accepted is **never** cancelled
  as never-submitted: the action is "await its terminal state".
- **R4-2a** *(rewritten in revision 3, round-1 B-1, round-2 B-1)* **Logical
  now, physical after the receipts.** A winning transition performs its
  **logical** obligations at once, whatever the state of any receipt —
  `VTRelease` releases the seat, `DeviceRemoved` and `Shutdown` withdraw
  outputs and terminalize protocol work (C.0: "prompt logical progress while an
  executor host call is outstanding"). Only its **physical** advancement —
  final `TEST_ONLY`, opening an fd, installing state, a topology request — waits
  for the driver's receipts that admission is closed, pre-submit work is
  cancelled, each Present is terminalized and quarantine is transferred **to
  this transition**. A receipt proves only what its own transition requested
  (round-3 B-1): when a transition is superseded, the new winner requests its
  own transfer — from wherever the resources now are — and waits for the
  receipt tagged with its own id; a late receipt tagged with a superseded
  transition is recorded against that transition and never counts toward the
  winner's gate. Receipts do not wait on the loser's own terminal state. A
  **failed** receipt leaves the affected resources in quarantine and the winner
  physically fenced — the device cannot install until the fd-family barrier
  (3d) — but never blocks the winner's logical obligations.
- **R4-3** An equal event coalesces per `REC-5`; a lower event only updates
  `LifecycleDesired` and cannot start work until the active transition is
  terminal.
- **R4-4** On terminalization, convergence revalidates every changed field and,
  if a still-valid field is unsatisfied and its prerequisite present, creates
  exactly one new transition of the highest-precedence unsatisfied kind
  (C.0 `REC-6`'s field→kind mapping: absent device → `DeviceRemoved`, …, output
  power targets → `DPMS`, unconsumed incident → `NormalRecovery`);
  `DeviceAddedOrReplaced` defers on seat, `VTAcquire` on device presence.
- **R4-5** Epoch (C.0 item 65): a lifecycle arrival first closes admission; a
  clean drain stays under the old epoch; coalesced events cause **one** bump;
  a forced abandonment bumps the epoch **before** it invalidates, so a delayed
  reply cannot publish across the boundary. Actions carry the tag of the
  transition they belong to; ordinary work carries the epoch with no
  transition id.
- **R4-6** Acknowledged outcomes (3a design §3.6/§3.7) map to arbiter inputs:
  current-tag `Completed` → the representative may become `Applied` (DPMS: when
  every projection on the device retired); explicit rejection → the state stays
  authoritative, the target stays desired, `Deferred(TopologyLatched(gen))` or
  `Deferred(ReadinessClosed)`, no retry under the same generation; completion loss → **Table U** for the row active at that moment (Task 3: only normal live operation and DPMS poison and create an incident; `VTRelease`, `DeviceRemoved` and `Shutdown` withdraw without one; `TopologyRebuild` transfers); stale-tag results never change installed state.
- **R4-7** While `Poisoned`, a DPMS change updates only logical power state:
  no action requests a KMS mutation (C.0 §10 lifecycle table, DPMS row). Its
  projection is **not retired**, so the representative is
  `Deferred(ReadinessClosed)` — never `Applied` — until a recovery installs
  and converges it (3d), or it is superseded by a newer DPMS generation or
  invalidated by shutdown, device removal or output removal *(rev 2, M-4)*.
  The logical DPMS-on is nevertheless the **resume boundary** of a paused
  incident (Task 3, F-2), so off → on on a poisoned device cannot deadlock.

**Tests:**

| Test | Scenario | Must fail under |
| --- | --- | --- |
| `c0_3a_every_pair_elects_by_precedence` | for **every ordered pair** of kinds (active, arriving): supersede iff arriving is higher; coalesce iff equal; else desired-only (items 57, 63) | **T12** supersede on equal kinds; **T13** let a lower kind supersede |
| `c0_3a_never_two_transitions` | every pair above, then every third kind: the arbiter never holds two | **T14** start the winner before the loser's terminal action |
| `c0_3a_logical_now_physical_after_receipts` | for `VTRelease`, `DeviceRemoved` and `Shutdown` superseding a DPMS whose host call is outstanding: the logical actions are emitted at once; no physical action until all four receipts; with one receipt delayed or failed the logical ones are still emitted and the physical ones never are; a receipt tagged with a superseded transition, arriving after a second supersession, does **not** open the newest winner's gate, which opens only on its own receipts | **T32** gate a logical action on a receipt; **T33** let physical advancement proceed with a failed quarantine receipt; **T43** count a retagged late receipt toward the newest winner |
| `c0_3a_submitted_is_never_cancelled_as_never_submitted` | supersession of a transition whose commit is accepted: the action is "await terminal", never "cancel" | **T15** emit cancel for an accepted commit |
| `c0_3a_convergence_selects_the_highest_unsatisfied` | for every subset of unsatisfied fields (with prerequisites present and absent), after terminalization: exactly one next transition of the highest kind, or `Deferred` with the right prerequisite (item 66) | **T16** pick the first unsatisfied field in declaration order |
| `c0_3a_mixed_arrivals_keep_every_lower_field` | *(round-2 M-1)* for **every active kind** (generated), arrivals of every mix of lower kinds — including two generations of each latest-wins field — in every order: after it terminalizes, the exact ledger (which ids are `SupersededBy` which, which remain representatives) and the exact sequence of successive winners match C.0 (item 63) | **T34** drop the reprobe field when a topology event arrives |
| `c0_3a_added_and_acquire_converge_in_either_order` | `DeviceAddedOrReplaced` and `VTAcquire` arriving in both orders both converge, all representatives terminal (item 66) | **T17** drop the seat prerequisite of `DeviceAddedOrReplaced` |
| `c0_3a_epoch_bumps_once_and_before_invalidation` | clean drain (no bump), N coalesced events (one bump), forced abandonment (bump emitted before the invalidation action) (item 65) | **T18** bump per coalesced event; **T19** invalidate before bumping |
| `c0_3a_ordinary_work_is_tagged_without_a_transition` | ordinary work issued while `Ready` carries the current epoch and `None`; after a lifecycle arrival bumps the epoch, a delayed reply of that ordinary work is stale and cannot promote (item 65) | **T35** tag ordinary work with the last transition's id; **T36** accept the delayed ordinary reply across the bump |
| `c0_3a_outcomes_map_to_dispositions` | every row of R4-6 for a DPMS transition, including a rejection that is never counted `Applied` and a stale success that never promotes | **T20** mark a rejected representative `Applied`; **T21** promote on a stale tag |
| `c0_3a_poisoned_dpms_is_logical_only` | `Poisoned` device, DPMS off then on: no KMS-mutation action, and the representative is `Deferred(ReadinessClosed)`, so the coordinator does not count it `Applied` | **T22** emit the DPMS commit request while `Poisoned`; **T37** mark the representative `Applied` |

## Task 5 — the coordinator

**Deliver:** the global state (`shutdown_requested` monotonic, `seat_target`
+ epoch, `protocol_dpms_level` + `dpms_epoch`), the `LifecycleEventId`
allocator, the projection of each global event to the per-device arbiters,
and the aggregation of the protocol DPMS request.

**Invariants:**
- **C-1** Every event updates the global snapshot first, then is projected;
  there is no queue.
- **C-2** DPMS: levels 1–3 are off, 0 on; one `dpms_epoch` per request; one
  representative per affected device; the same epoch across devices.
- **C-3** The protocol request is `Applied` only when **every** device's
  representative is `Applied` or was invalidated by output removal; a
  representative that ended any other way never counts (3a design rev 2).
- **C-4** `shutdown_requested` never goes back to false.
- **C-5** *(rev 3, round-2 B-3)* A completion loss reported by the driver is
  an event like any other: the coordinator allocates its `LifecycleEventId`
  (kind `NormalRecovery`) and projects it to that device's arbiter, which
  hands it to Task 3's Table U. Task 3's tests supply the id directly; this
  task proves the coordinator is the only allocator on that path.

**Tests:**

| Test | Scenario | Must fail under |
| --- | --- | --- |
| `c0_3a_dpms_projects_one_epoch_to_every_device` | three devices, off: one epoch, one representative each, levels 1/2/3 all off | **T27** project level 1 as on |
| `c0_3a_protocol_applied_only_when_every_device_applied` | device A applied, B rejected (`Deferred`), C removed-output invalidation: not `Applied`; then B applied: `Applied` | **T28** count any terminal disposition toward `Applied` |
| `c0_3a_shutdown_is_monotonic` | shutdown, then any event: still requested | **T29** let a seat event clear it |
| `c0_3a_loss_report_gets_a_coordinator_event_id` | the driver reports a loss on device A: the coordinator allocates exactly one event id, only A's arbiter receives it, and it becomes the incident's representative | **T44** let the device arbiter mint the event id itself |
| `c0_3a_loss_reaches_table_u_through_the_arbiter` | *(rev 5, round-4 M-1)* for **every** Table U row (generated): the loss is reported to the coordinator, projected to the device's arbiter while that row is active, and the arbiter's resulting actions, incident fate and event disposition equal Table U's for that row — the handoff, not the table alone; for rows that create an incident, the representative's disposition is then driven to each U-1b outcome and asserted | **T49** project the loss as a plain lower-priority `NormalRecovery` event that waits behind the active row instead of entering Table U |
| `c0_3a_devices_converge_independently` | a slow device and a fast one under the same epoch: the fast one's arbiter state is final before the slow one's | **T30** make projection wait for every device before any applies |

## Gate (every task)

`cargo +nightly fmt`; `cargo clippy --all-targets -- -D warnings`, again with
`--features tcp-transport` and with `--features xdmcp`;
`cargo test -p yserver --lib c0_3a_` (report the count);
`cargo test -p yserver --test compile_fail` from Task 1 on;
`cargo test -p yserver --lib` (report passed/failed/ignored). Report exact
counts transcribed from the output. Apply each task's mutations **by line**,
confirm the mutated build **compiled**, name the test that failed, and restore
exactly; a mutation that does not compile is not caught. A mutation must
**replace** the production branch, not run beside it.

## Limits

No production caller until plan 3a-ii (stated above). No executed transition,
no fd, no commit. The stage 3 kinds other than DPMS have no production event
source until 3c/3d; their rows are proven here as decisions only.
