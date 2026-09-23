# Stage 3a-i — the pure lifecycle arbiter

> **Implementer:** codex (model `gpt-6-luna`, reasoning effort `xhigh`), run **without sandbox** (`--sandbox danger-full-access`, user-authorized) with `< /dev/null`. Hard rules, restated in every prompt: **no git write commands** (the coordinator verifies and commits); this plan needs **no GPU and no `#[ignore]` test** — run none of them, never `_drm`, never `render_acceptance`, never an unfiltered `--ignored`; no deletes outside the worktree; remove temporary instrumentation before finishing. **You write the implementation and the tests**; this plan gives the interfaces, the invariants, the named tests with the scenario each must exercise, and the mutations each must catch. Execute tasks in order, one at a time; stop with the tree dirty after each task. **Do not ask for approval inside a run** — if the plan leaves a real design choice open, or something it states does not hold in the code or in C.0, stop and report it (F8); never silently substitute a test shape.

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
protocol-output removal, newer generation) plus the three external boundaries
`REC-6` names for an invalidated incident; `Prerequisite` — seat released,
device absent, and the two stage 3a failure prerequisites
`TopologyLatched(generation)` and `ReadinessClosed`; `DeviceLifecycleState`
— the nine §6.4 states, `Recovering` carrying its `RecoveryId`; `RecoveryId`
— its own counter, with **no** conversion from or to `LifecycleTransitionId`;
`TransitionTag` — incarnation, `LifecycleEpochId`, `LifecycleTransitionId`
(the incarnation as an opaque value supplied by the caller, decision 2).

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
  become `Applied`, an `AbsorbedBy*`, or `Invalidated`.
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
| `c0_3a_deferred_reaches_one_terminal` | a `Deferred(SeatReleased)` target, then seat acquired / shutdown / newer generation: each path ends in exactly one terminal disposition | **T9** leave the representative `Deferred` after the prerequisite returns |
| `c0_3a_projection_follows_the_output_domain` | global off, then an output is added (inherits off before install), then another is removed (only its projection invalidated, once) (item 66) | **T10** add the new output with target on; **T11** invalidate every projection on one removal |

## Task 3 — the arbiter: `REC-4` precedence, supersession, convergence, epoch

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
  `Deferred(ReadinessClosed)`, no retry under the same generation; completion
  loss → §6.4 `Poisoned`, admission closed, transition terminal, `REC-6`
  outcome recorded; stale-tag results never change installed state.
- **R4-7** While `Poisoned`, a DPMS change updates only logical power state:
  no action requests a KMS mutation (C.0 §10 lifecycle table, DPMS row).

**Tests:**

| Test | Scenario | Must fail under |
| --- | --- | --- |
| `c0_3a_every_pair_elects_by_precedence` | for **every ordered pair** of kinds (active, arriving): supersede iff arriving is higher; coalesce iff equal; else desired-only (items 57, 63) | **T12** supersede on equal kinds; **T13** let a lower kind supersede |
| `c0_3a_never_two_transitions` | every pair above, then every third kind: the arbiter never holds two | **T14** start the winner before the loser's terminal action |
| `c0_3a_submitted_is_never_cancelled_as_never_submitted` | supersession of a transition whose commit is accepted: the action is "await terminal", never "cancel" | **T15** emit cancel for an accepted commit |
| `c0_3a_convergence_selects_the_highest_unsatisfied` | for every subset of unsatisfied fields (with prerequisites present and absent), after terminalization: exactly one next transition of the highest kind, or `Deferred` with the right prerequisite (item 66) | **T16** pick the first unsatisfied field in declaration order |
| `c0_3a_added_and_acquire_converge_in_either_order` | `DeviceAddedOrReplaced` and `VTAcquire` arriving in both orders both converge, all representatives terminal (item 66) | **T17** drop the seat prerequisite of `DeviceAddedOrReplaced` |
| `c0_3a_epoch_bumps_once_and_before_invalidation` | clean drain (no bump), N coalesced events (one bump), forced abandonment (bump emitted before the invalidation action) (item 65) | **T18** bump per coalesced event; **T19** invalidate before bumping |
| `c0_3a_outcomes_map_to_dispositions` | every row of R4-6 for a DPMS transition, including a rejection that is never counted `Applied` and a stale success that never promotes | **T20** mark a rejected representative `Applied`; **T21** promote on a stale tag |
| `c0_3a_poisoned_dpms_is_logical_only` | `Poisoned` device, DPMS off then on: no KMS-mutation action | **T22** emit the DPMS commit request while `Poisoned` |

## Task 4 — `REC-6`: the recovery-fate matrix and `REC-1` accounting

**Deliver:** the incident record (its `RecoveryId`, its one-attempt budget, its
state) and the function that, for a winning kind and an active incident,
returns the matrix outcome.

**Invariants (C.0 `REC-6` table, lines ~918–935, and `REC-1`):**
- **R6-1** Exactly the table: `Shutdown`, `DeviceRemoved`, `VTRelease` →
  `Invalidated` with that reason; `DeviceAddedOrReplaced`, `VTAcquire`,
  `AdministrativeReprobe`, `IdentityChangingHotplug` → old incident invalidated
  by that boundary and **at most one fresh** `RecoveryId` if recovery is still
  required; same-identity `TopologyRebuild` → the **same** incident and its
  remaining budget transferred; `DPMS` → paused (off defers it on
  `dpms_target = On`, on resumes it), never consumed, cloned or revived;
  `NormalRecovery` → continued, equal events absorbed.
- **R6-2** No path both invalidates and transfers an id, allocates two ids,
  lets DPMS revive `RecoveryFailed`, or spends an attempt twice (item 67).
- **R6-3** `REC-1`: one automatic attempt per incident; any failure or
  unknown during it → `RecoveryFailed`; timers, DPMS, client traffic and
  queued intents cannot create another attempt.

**Tests:**

| Test | Scenario | Must fail under |
| --- | --- | --- |
| `c0_3a_recovery_matrix_is_total` | every winning kind × an active incident (and × `RecoveryFailed`): the exact outcome of R6-1 (item 67) | **T23** transfer the incident on `IdentityChangingHotplug`; **T24** let DPMS allocate a fresh id |
| `c0_3a_no_double_fate` | every row: the outcome is never both invalidated and transferred, and at most one id is allocated | **T25** allocate a fresh id and also transfer the old one on `VTAcquire` |
| `c0_3a_one_attempt_per_incident` | an attempt fails, then DPMS on, a timer, and repeated normal-recovery events: no second attempt, state `RecoveryFailed` | **T26** let DPMS-on resume a `RecoveryFailed` incident |

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

**Tests:**

| Test | Scenario | Must fail under |
| --- | --- | --- |
| `c0_3a_dpms_projects_one_epoch_to_every_device` | three devices, off: one epoch, one representative each, levels 1/2/3 all off | **T27** project level 1 as on |
| `c0_3a_protocol_applied_only_when_every_device_applied` | device A applied, B rejected (`Deferred`), C removed-output invalidation: not `Applied`; then B applied: `Applied` | **T28** count any terminal disposition toward `Applied` |
| `c0_3a_shutdown_is_monotonic` | shutdown, then any event: still requested | **T29** let a seat event clear it |
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
