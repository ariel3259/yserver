# Stage 3b-ii — the RANDR protocol on the Owner: gate, publication, bound

> **Implementer:** codex (model `gpt-6-luna`, reasoning effort `xhigh`; `max` from the first send-back), run with `< /dev/null`. Hard rules, restated in every prompt: **no git write commands** (the coordinator verifies and commits); of the `#[ignore]` tests run only this plan's filters, each by its own command — `c0_3bii_`, `c0_3bi_`, `c0_3aii_`, `c0_adm` in `yserver`, and the whole `yserver-core` suite — with `--include-ignored` only when the prompt records the user's GPU approval, otherwise without it; **never** `_drm` tests, `render_acceptance`, an unfiltered `--ignored`, or anything that performs a modeset or takes DRM master; the hardware additions of Task 5 are **written, never run**, by the implementer; no deletes outside the worktree; remove temporary instrumentation before finishing; never edit `docs/status.md`. **You write the implementation and the tests**; this plan gives the interfaces, the invariants, the named tests with the scenario each must exercise, and the mutations each must catch. Execute tasks in order, one at a time; stop with the tree dirty after each task. **Do not ask for approval inside a run** — if the plan leaves a real design choice open, or something it states does not hold in the code or in C.0, stop and report it (F8); never silently substitute a test shape, never weaken an existing assertion.

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
   flight, or is not the FIFO head at all. Queries never block.
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
modeset answers `ContinuesWithoutRequester`. On disconnect the gate keeps a
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
called where the loop drains ready CRTC configs). The backend's `REC-4`
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
| `c0_3bii_requesterless_on_an_idle_gate` | gate empty: published at once, `lastConfigTime` updated per its rule | **G19** queue it behind nothing |

## Task 5 — the protocol-order differential and the hardware client

**Deliver:** umbrella §4.1 layer 1: drive the core request path with a Legacy
and an Owner `KmsBackend` fixture (3b-i's Owner path, live Vulkan fixture),
two client connections (requester and listener), and compare the bytes
written to each connection — reply, status, events, in order. Cases: the
idempotent request; supersession by a `REC-4` event; two concurrent clients
(including the MATE sequence); cross-device and cross-transport order; the
requester disconnecting while parked; each `E` stage expired (via the stub
executor) and `Q`; the VT released; a requester-less publication and the
supersession sequence of design §7.5. Identical except the named exceptions
(design §8.4), each asserted as the named difference. The hardware test of
3b-i gains an in-test protocol client that issues the same `SetCrtcConfig`
sequence through the core and checks its reply bytes.

| Test | Scenario | Must fail under |
| --- | --- | --- |
| `c0_3bii_protocol_order_differential_vulkan` | the case list above, Legacy vs Owner bytes | **G20** answer `Success` before promotion on Owner (design mutation 1, protocol level) |
| `c0_3bii_named_exceptions_are_the_only_differences_vulkan` | the differential's diff set equals the named exceptions reachable in the script | **G21** introduce an unnamed difference (for example drop one change notification on Owner) |
