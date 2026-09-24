# Stage 3b-ii — the RANDR protocol on the Owner: gate, publication, bound

> **Implementer:** codex (model `gpt-6-luna`, reasoning effort `xhigh`; `max` from the first send-back), run with `< /dev/null`. Hard rules, restated in every prompt: **no git write commands** (the coordinator verifies and commits); of the `#[ignore]` tests run only this plan's filters, each by its own command — `c0_3bii_`, `c0_3bi_`, `c0_3aii_`, `c0_adm` in `yserver`, and the whole `yserver-core` suite — with `--include-ignored` only when the prompt records the user's GPU approval, otherwise without it; **never** `_drm` tests, `render_acceptance`, an unfiltered `--ignored`, or anything that performs a modeset or takes DRM master; the hardware additions of Task 5 are **written, never run**, by the implementer; no deletes outside the worktree; remove temporary instrumentation before finishing; never edit `docs/status.md`. **You write the implementation and the tests**; this plan gives the interfaces, the invariants, the named tests with the scenario each must exercise, and the mutations each must catch. Execute tasks in order, one at a time; stop with the tree dirty after each task. **Do not ask for approval inside a run** — if the plan leaves a real design choice open, or something it states does not hold in the code or in C.0, stop and report it (F8); never silently substitute a test shape, never weaken an existing assertion.

**Revision 6 (2026-09-24, coordinator)** — codex round 5 (1 blocking,
`../findings/2026-09-24-stage-3b-ii-plan-review-round5.md`): the plan
contradicted the design's "queries never wait". Design revision 11 names the
forced reprobe: `GetScreenResources` joins the gate only behind an
install-capable mutation (named exception 6); the plan follows it.

**Revision 5 (2026-09-24, coordinator)** — codex round 4 (0 blocking, 1
major, `../findings/2026-09-24-stage-3b-ii-plan-review-round4.md`):
`KillClient`'s inline removal of an **in-flight** requester applies the same
abandonment as a disconnect (Task 2). Round 4 reviewed revision 3 again: the
coordinator's revision 4 edits had failed to apply and were re-applied here
together. Review loop closed at the first round without a blocking finding.

**Revision 4 (2026-09-24, coordinator)** — codex round 3
(`../findings/2026-09-24-stage-3b-ii-plan-review-round3.md`: 1 blocking, 1
major): `GetScreenResources`, which forces a connector reprobe that can
publish, joins the gate as a synchronous member (B-1, Task 1); the pre-C.0
oracle is the core's existing RANDR byte tests, which 3b-ii may not change,
plus stage 5's golden rerun — the user decided on 2026-09-22 that the
captured golden stays outside the repository (M-1, Task 5).

**Revision 3 (2026-09-24, coordinator)** — codex round 2
(`../findings/2026-09-24-stage-3b-ii-plan-review-round2.md`: 1 blocking, 1
major, both verified): a generation reset (and `-terminate`) waits for an
install-capable mutation to reach its terminal result before it snapshots
backend state (B-1, Task 2; design revision 10 §7.2); the mixed-server
script uses three requester connections (M-1, Task 5).

**Revision 2 (2026-09-24, coordinator)** — codex round 1
(`../findings/2026-09-24-stage-3b-ii-plan-review-round1.md`: 2 blocking, 1
major, all verified): a requester-less publication wakes the core on enqueue
(B-1, Task 4); every client-removal path — including `KillClient`'s inline
removal — prunes the gate (B-2, Task 2); Task 5 carries the compound
wire-level scripts (M-1).

**Revision 1 (2026-09-24, coordinator).**

**Goal:** the six RANDR obligations of the umbrella (§3b) hold for Owner and
Legacy alike: one named component orders every RANDR mutation, `Success`
means installed, an installed change is published even if its requester left,
every parked request is answered within `Q + E` (+ `L` in a mixed server), and
changes no request caused have an ordered publication path.

**Prerequisite:** plans 3b-i-1 and 3b-i-2 accepted. Tasks 1–4 are core-side
and are exercised with `RecordingBackend`
(`crates/yserver-core/src/backend/recording.rs:203`); they do not need the
Owner path. Task 5's protocol-order differential needs it.

**Authority:** stage 3b design
`docs/superpowers/specs/2026-09-24-phase-c0-stage-3b-modeset-and-randr-design.md`
revision 9, §7 and §8.2; the umbrella §3b obligations and §4.1 layer 1.

## Design decisions this plan fixes

1. **`RandrMutationGate`** lives in `yserver-core`'s core loop beside
   `PendingBackendRequests` (`core_loop/run.rs:557`). It owns: the in-flight
   mutation (at most one), the FIFO of waiting client ids in arrival order, each
   waiter's arrival instant, and the publication continuation of the in-flight
   mutation.
2. **Blocking reuses the ready ring**: `client_is_blocked` (`run.rs:564`)
   becomes true also for a client whose **next** deferred request is a RANDR
   mutation and which is not the gate's FIFO head while a mutation is in
   flight, or is not the FIFO head at all. Queries never block, except
   `GetScreenResources` behind an install-capable mutation (Task 1, design
   revision 11 §7.1).
3. **Test names**: `c0_3bii_` in both crates; core tests use
   `RecordingBackend` extended with a scripted `begin_crtc_config`
   (`Applied`/`Pending`, controllable completion, controllable stall).
4. **Gates** as in 3b-i-1, plus `cargo test -p yserver-core` in full at every
   task.

## Review Focus

1. A client whose next request is a mutation, with ordinary requests queued
   behind it, while the gate is busy: its ordinary requests also wait
   (per-client FIFO), other clients' ordinary requests do not (Task 1
   `c0_3bii_gate_blocks_only_the_mutating_client`).
2. The requester disconnects while it is the gate's FIFO head but not yet
   admitted (Task 2 `c0_3bii_waiting_head_disconnects`).
3. Two parkable requests queued behind a slow one: both expire at their own
   `Q`, in order (Task 3 `c0_3bii_two_waiters_expire_in_order`).
4. A requester-less publication arriving while the gate is empty — it
   publishes at once (Task 4 `c0_3bii_requesterless_on_an_idle_gate`).
5. The idempotent `SetCrtcConfig` of MATE's re-assert behind a real change
   from another client — evaluated after the change is published (Task 1
   `c0_3bii_mate_reassert_behind_a_change`).

---

## Task 1 — the gate

**Deliver:** `RandrMutationGate` and its blocking (decisions 1–2). The
mutation set is enumerated from `handle_randr_request`
(`core_loop/process_request.rs:2810`): a request is a mutation iff its arm can
change CRTC configuration, the screen size, the primary output or a provider
relationship — at least `SetScreenConfig`, `SetScreenSize`, `SetCrtcConfig`,
`SetOutputPrimary`, `SetProviderOutputSource`, `SetProviderOffloadSink`,
`SetPanning`, `SetCrtcTransform`; `SetCrtcGamma` and the output/provider
property requests are excluded unless their arm changes configuration. The
implementer states the decision for **every** RANDR opcode in the report.
*(Rev 4, B-1; narrowed in rev 6 to design revision 11.)* `GetScreenResources` forces a connector reprobe
(`reprobe_connectors`, `process_request.rs:2939`) that can rebuild RANDR state
and emit notifications before its reply (`render/backend.rs:24457`,
`:10348`). It is therefore a **synchronous gate member while the gate holds an
install-capable mutation** (a dispatched Owner modeset) — otherwise, including
behind a Legacy PRIME probe, it runs at once as today: while such a mutation is
in flight it waits in the FIFO like a synchronous mutation (never expired), so
its probe and any publication it makes happen after the in-flight mutation's
publication — Legacy's serial order. `GetScreenResourcesCurrent` and the other
queries stay pure queries.
Validation of an admitted mutation runs only after admission, against the
published state. A mutation leaves the gate when its publication completes,
or at once when it ends without one. The pure-Legacy server has nothing in
flight except a PRIME-probe `Pending`, so its behaviour changes only by the
named exception (the concurrent PRIME probe no longer goes stale).

| Test | Scenario | Must fail under |
| --- | --- | --- |
| `c0_3bii_one_mutation_in_flight` | client A's `SetCrtcConfig` parked (`Pending`), client B sends `SetCrtcConfig`: B's request is not processed until A's publication completed | **G1** admit a second mutation (design mutation 5) |
| `c0_3bii_gate_is_fifo_by_arrival` | A in flight; C then B reach the gate; the ready ring would pick B first: C is admitted first | **G2** admit in ready-ring order (design mutation 16) |
| `c0_3bii_gate_blocks_only_the_mutating_client` | A in flight; B's next request is a mutation with a `GetScreenResources` behind it; D sends only queries: D is served at once, B's query waits behind B's mutation | **G3** block every client |
| `c0_3bii_queries_read_published_state` | A in flight (installed in the backend but not yet published): a `GetCrtcInfo` from B answers the published, old state | **G4** publish at backend completion instead of in the gate |
| `c0_3bii_mate_reassert_behind_a_change` | A changes HDMI-2 to 1280x720 (parked); B sends the 1920x1080 re-assert: after A publishes, B is evaluated as a real change back (not idempotent) and its events follow A's | **G5** validate or compare B before A publishes |
| `c0_3bii_forced_reprobe_waits_for_the_gate` | *(rev 4)* A's modeset in flight; B sends `GetScreenResources` while the recording backend's reprobe would report a changed connector: B's reply and the reprobe's notifications come after A's publication; a `GetScreenResourcesCurrent` from C is served at once from the published state | **G6b** treat `GetScreenResources` as a pure query |
| `c0_3bii_forced_reprobe_does_not_wait_for_a_prime_probe` | *(rev 6)* a Legacy PRIME probe parked: `GetScreenResources` from another client runs at once, as today | **G6c** make it wait behind any in-flight mutation |
| `c0_3bii_every_opcode_classified` | a table test over every RANDR minor opcode asserting its mutation/query classification as reported | **G6** drop one mutation from the set |

## Task 2 — publication outlives the requester (obligation 3)

**Deliver:** `ParkedCrtcConfig` (`core_loop/run.rs:548`) is split into the
**publication** (owned by the gate: the RANDR state rebuild with the
request's `set_time`, the change notifications to selecting clients, the
screen-resize notifications) and the **reply** (owned by the client). The
`Backend` trait gains
`fn abandon_crtc_config_requester(&mut self, token) -> RequesterAbandon` with
`RequesterAbandon::{Cancelled, ContinuesWithoutRequester}`: the Legacy PRIME
probe and an undispatched Owner modeset answer `Cancelled`; a dispatched Owner
modeset answers `ContinuesWithoutRequester`. *(Rev 2, B-2.)* **Every** client-removal
path updates the gate — `disconnect_with_pending_cleanup` (`run.rs:828`) and
the inline removal `KillClient` performs on another client (detected at
`run.rs:884` by the client count) — either by calling the same gate cleanup
or by pruning, before the next admission, every FIFO entry and in-flight
requester whose client no longer exists. For an in-flight requester the prune
applies **the same abandonment** as a disconnect
(`abandon_crtc_config_requester`): only the reply attachment is removed, and a
`ContinuesWithoutRequester` publication stays in the gate *(rev 5)*. *(Rev 3, B-1.)* A **generation reset** or `-terminate`
(`core_loop/run.rs:1836`, `core_loop/reset.rs`) triggered while the gate holds
an install-capable mutation (a `ContinuesWithoutRequester` one) is deferred
until that mutation's terminal result — bounded by `E` — and only then
cancels the remaining tokens and snapshots backend state for the new
generation; the old mutation's reply and events are dropped with its
generation. On disconnect the gate keeps a
`ContinuesWithoutRequester` publication and stays occupied until the result;
if installed it is published to every remaining client and only the reply is
dropped. `Success` is answered only from `Ok(true)` or the idempotent
`Ok(false)`; every error answers `Failed` and publishes nothing (obligation 2).
`lastSetTime` takes the request's timestamp without a monotonicity rule.

| Test | Scenario | Must fail under |
| --- | --- | --- |
| `c0_3bii_disconnect_after_dispatch_still_publishes` | A parked with `ContinuesWithoutRequester`, A disconnects, the backend completes `Ok(true)`: listener L receives the change notifications, nothing is written for A, the gate then admits the next waiter | **G7** drop the publication on disconnect (design mutation 6) |
| `c0_3bii_disconnect_before_dispatch_cancels` | the backend answers `Cancelled`: no publication, the gate frees at once | **G8** keep the gate occupied after a cancel |
| `c0_3bii_waiting_head_disconnects` | B is the FIFO head waiting behind A and disconnects: B leaves the FIFO, the next waiter keeps its place | **G9** leave the departed client in the FIFO |
| `c0_3bii_reset_waits_for_an_install_capable_mutation` | *(rev 3, B-1)* the last client's dispatched modeset continues without its requester; the reset trigger fires (last client left): the reset does not cancel the token or snapshot backend state until the result is terminal; after `Ok(true)` the new generation's RANDR state is seeded from the installed topology, and nothing of the old client (reply, events) reaches the new generation; with `-terminate` the server exits only after the terminal result | **G9c** cancel the token and seed at once (today's `reset.rs` step order) |
| `c0_3bii_killclient_of_a_dispatched_requester_still_publishes` | *(rev 5)* A's modeset dispatched (`ContinuesWithoutRequester`); D kills A with `KillClient`; the backend completes `Ok(true)`: the listener receives A's change notifications, nothing is written for A, and the next waiter is admitted after the publication | **G9d** prune the whole in-flight entry on inline removal |
| `c0_3bii_killclient_removes_a_waiting_head` | A in flight, B the waiting head, C behind; D kills B with `KillClient`; A completes: C is admitted next and B's entry is gone | **G9b** clean the gate only in `disconnect_with_pending_cleanup` |
| `c0_3bii_failed_publishes_nothing` | the backend completes `Err(..)`: reply status 3, timestamp unchanged, no notification | **G10** publish on error |
| `c0_3bii_last_set_time_can_go_backward` | two successive changes with a decreasing client timestamp: `lastSetTime` follows the request, as Legacy | **G11** clamp `lastSetTime` monotonic |

## Task 3 — the bound (obligation 5)

**Deliver:** the gate queue deadline `Q = 30 s` from each waiter's arrival,
with a timeout wake of the core loop (the loop's next wakeup takes the
earliest waiter deadline into account). At `Q` a parkable waiter
(`SetCrtcConfig`) is taken out of the FIFO and answered **without
state-dependent validation**: only the stateless request checks (length and
field format) run; a request that passes them answers `Failed`
(`GateExpired`) with the current published timestamp and no publication; a
malformed one gets its stateless error. Synchronous mutations are never
expired. After a synchronous mutation returns, every queued deadline is
serviced **before** the next admission. The execution bound `E` is the
backend's (3b-i); the gate adds none.

| Test | Scenario | Must fail under |
| --- | --- | --- |
| `c0_3bii_waiter_expires_at_q` | A parked for longer than `Q` (the recording backend holds it); B's `SetCrtcConfig` waits: at `Q` B answers status 3 with the published timestamp, without a backend call; a timer wake, not I/O, triggers it | **G12** no timeout wake |
| `c0_3bii_expiry_is_stateless` | B's request would be `BadMatch` against the published state but valid after A publishes: at `Q` B answers `Failed`, not `BadMatch`; a malformed B still gets `BadLength` | **G13** run state-dependent validation at expiry (design mutation 18) |
| `c0_3bii_sync_mutation_is_never_expired` | a `SetScreenSize` waits past `Q` behind A: it is not expired and executes when A publishes | **G14** apply `Q` to synchronous mutations |
| `c0_3bii_deadlines_first_after_a_sync_mutation` | a synchronous mutation that stalls the loop (the recording backend sleeps inside it) past C's `Q`: C is answered on the first iteration after it returns, before any other waiter is admitted | **G15** admit the next waiter first (design mutation 20) |
| `c0_3bii_two_waiters_expire_in_order` | B and C wait behind a slow A: each expires at its own `Q`, in arrival order | **G16** expire all at the first deadline |

## Task 4 — changes no request caused (obligation 6)

**Deliver:** the gate accepts **requester-less publications** from the
backend (a new `Backend` drain, `fn drain_requesterless_publications(&mut self) -> Vec<RequesterlessPublication>`,
called where the loop drains ready CRTC configs). *(Rev 2, B-1.)* The
backend sends `Message::CrtcConfigReady` (or a dedicated publication wake)
whenever it enqueues a requester-less publication, and the core drains
publications on that wake even when no CRTC token is ready
(`core_loop/run.rs:1603` today drains only tokens). The backend's `REC-4`
events never pass through the gate. A requester-less publication waits only
behind an in-flight mutation that can still install (a dispatched Owner
modeset); otherwise it publishes at once, before any waiter is admitted.
`lastConfigTime` keeps its rule. A server with no Owner device keeps today's
publication path (the drain is empty). A test producer in `RecordingBackend`
stands in for 3c/3d.

| Test | Scenario | Must fail under |
| --- | --- | --- |
| `c0_3bii_requesterless_waits_behind_a_dispatched_modeset` | A dispatched (`ContinuesWithoutRequester` semantics), a requester-less publication arrives: it publishes after A's publication | **G17** publish it before A |
| `c0_3bii_requesterless_does_not_wait_for_a_superseded_one` | A parked before dispatch; the producer supersedes A (A completes `Err(Superseded)`) and publishes: the requester-less publication goes out, then A's `Failed` reply, with nothing published for A | **G18** hold the event at the gate (design mutation 15) |
| `c0_3bii_requesterless_on_an_idle_gate` | gate empty, no CRTC token pending: the producer enqueues a publication from outside a request; the loop wakes and publishes it at once, `lastConfigTime` updated per its rule | **G19** queue it behind nothing; **G19b** drain publications only when a CRTC token is ready |

## Task 5 — the protocol-order differential and the hardware client

**Deliver:** umbrella §4.1 layer 1: drive the core request path with a Legacy
and an Owner `KmsBackend` fixture (3b-i's Owner path, live Vulkan fixture),
client connections — one requester and one listener for the single-request
cases, and *(rev 3, M-1)* **three independent requester connections plus a
listener** for the concurrent and mixed-server scripts, with byte assertions
per connection — and compare the bytes
written to each connection — reply, status, events, in order. Cases: the
idempotent request; supersession by a `REC-4` event; two concurrent clients
(including the MATE sequence); cross-device and cross-transport order; the
requester disconnecting while parked; each `E` stage expired (via the stub
executor) and `Q`; the VT released; a requester-less publication and the
supersession sequence of design §7.5; *(rev 2, M-1)* and the compound
scripts of design §8.2, each at wire level: FIFO admission against ready-ring
order; `Q` expiry behind a predecessor whose installation changes the
waiter's validation (answer `Failed` either way), a malformed waiter (its
stateless error), a synchronous waiter behind a slow mutation (not expired);
and the mixed-server sequence — Owner A in flight, Legacy B and Owner C
queued, B stalled past C's `Q`, C answered on the first iteration after B
returns. *(Rev 4, M-1.)* Relative parity between the two fixtures is not enough
on its own: both run the modified core. The pre-C.0 oracle is (a) the
existing `yserver-core` tests that assert RANDR reply and event bytes (the
implementer lists them) — 3b-ii may not change any of them, and a needed
change is an F8 — and (b) stage 5's layer-3 rerun against the user's golden
capture, which by the user's decision of 2026-09-22 stays outside the
repository. Identical except the named exceptions
(design §8.4), each asserted as the named difference. The hardware test of
3b-i gains an in-test protocol client that issues the same `SetCrtcConfig`
sequence through the core and checks its reply bytes.

| Test | Scenario | Must fail under |
| --- | --- | --- |
| `c0_3bii_protocol_order_differential_vulkan` | the case list above, Legacy vs Owner bytes | **G20** answer `Success` before promotion on Owner (design mutation 1, protocol level) |
| `c0_3bii_named_exceptions_are_the_only_differences_vulkan` | the differential's diff set equals the named exceptions reachable in the script | **G21** introduce an unnamed difference (for example drop one change notification on Owner) |
