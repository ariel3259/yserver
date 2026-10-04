# Phase C.0 stage 3d — recovery, quarantine and shutdown

**Status:** revision 2 (design review round 1: M-1 the invisibility contract
scoped, requests during recovery parked, late executor results). Design
approved by the user section by section on 2026-10-04. One spec, two plans
(3d-i, 3d-ii).

**Related specifications:**

- `docs/superpowers/specs/2026-08-26-phase-c0-atomic-kms-migration-design.md`
  (C.0): §6.4 device lifecycle matrix and `REC-1`..`REC-6`, §10 (incarnation
  poison, recovery, the lifecycle table), §17/§18.
- `docs/superpowers/specs/2026-09-22-phase-c0-stage-3-lifecycle-design.md`
  (stage 3 umbrella): §3 "3d — Recovery, quarantine and shutdown", §4
  client-visible contract and differential gate, §5 evidence regime.
- `docs/superpowers/specs/2026-09-26-phase-c0-stage-3c-vt-and-hotplug-design.md`
  (3c): the acquire reinstall this stage reuses, and its §8 items carried to 3d.

## 1. What 3d is

3d is the exit of stage 3. Every §6.4 state that earlier sub-stages only typed
is executed, and the §6.4 matrix becomes total: **no resource is retained
without an owner**. Production remains `Legacy` until stage 5, so every path is
exercised by fixtures and by card1 hardware tests.

What already exists: the nine `DeviceLifecycleState` values
(`kms/owner/lifecycle/values.rs`), the `REC-6` incident machinery
(`kms/owner/lifecycle/recovery.rs`, 3a), the `Poisoned` entry on a completion
loss (`arbiter.rs`, 3a), logical withdrawal and `DeviceRemoved` (3c), the VT
acquire reinstall from scratch (3c-i, `begin_acquire_episode`), and an
unsupervised shutdown (`lib.rs` → `shutdown_destroy_drawables`).

## 2. Decisions (user, 2026-10-04)

1. **Split:** one spec, two plans — **3d-i** recovery (`Poisoned` →
   `Recovering` → `Ready`/`RecoveryFailed`, `ExecutorStalled`) and **3d-ii**
   quarantine transfer, `Removed` teardown and orderly shutdown
   (`ShutdownExecutorStalled`).
2. **A successful recovery does not change what clients see of the
   topology.** Outputs stay published while a recovery runs; no RANDR
   notification is sent and read-only topology replies are unchanged unless the
   reinstalled topology differs. Logical withdrawal happens only when the
   executor stalls (`ExecutorStalled`) or the recovery fails
   (`RecoveryFailed`). The visible effect of a successful recovery is a frozen
   screen until the reinstall completes. Requests that arrive meanwhile follow
   decision 2a; protocol work the completion loss affected is terminalized as
   C.0 §10 requires.
   - **2a (user, after review round 1):** an `RRSetCrtcConfig` (or other RANDR
     mutation) arriving while the device is `Poisoned`/`Recovering` **waits in
     the existing `RandrMutationGate`** until the recovery terminates: on
     `Ready` it executes normally; on `RecoveryFailed` it is validated against
     the withdrawn state. The gate's existing `Q` = 30 s bound still applies.
     A Present affected by the completion loss is terminalized (as during a VT
     release). These are the recovery's client-visible differences from a run
     without the failure and are listed as a named exception of the umbrella §4
     client contract: a delayed RANDR reply, and the terminalized Presents.
3. **Amendment to C.0 §10 — bounded administrative reprobe.** RANDR's forced
   reprobe (`RRGetScreenResources`) is `AdministrativeReprobe` (3c). After a
   `RecoveryFailed`, it may create **at most one** fresh recovery attempt; if
   that attempt fails again, a further forced reprobe creates none until an
   actual hotplug identity change, a VT reacquire or a restart. Reason: clients
   other than users call `RRGetScreenResources`, and each attempt reopens the
   device and modesets; an unbounded reading would let a polling client cause
   repeated flicker on a failed device.
4. **Shutdown teardown deadline: 3 seconds** for reaping every executor/helper
   child after the seat is released. At the deadline the server exits with the
   unreaped lease recorded (`ShutdownExecutorStalled`).
5. **`REC-1` is kept**, deliberately beyond the references: Xorg's modesetting
   driver performs KMS calls synchronously in-process with no deadline (a call
   stuck in the kernel blocks the server itself), and wlroots has no recovery
   (a never-completing flip freezes the output; a DRM event failure destroys the
   backend). C.0's isolated executor is what makes a single automatic recovery
   possible.
6. **Recovery reuses the 3c-i acquire reinstall on a fresh incarnation**
   (approach A). Rejected: modelling recovery as an internal remove+add (it
   publishes RANDR changes, contradicting decision 2) and a separate recovery
   path (it duplicates the reinstall logic already proven on hardware).
7. **Codex may run the 3d hardware tests it writes** (process, §6).

## 3. 3d-i — recovery

### 3.1. Flow

A completion loss or mechanism breach already moves the device to `Poisoned`
(3a): admission closed, the record quarantined. 3d-i executes the exit:

1. **Incident.** The poisoning allocates exactly one `RecoveryId` through the
   existing `REC-6` machinery. It is never derived from a
   `LifecycleTransitionId`.
2. **Old fd-family barrier.** Stop admission and alias creation, detach every
   submitter and event reader, request executor/helper termination, and reap
   every lease. Only when the `IncarnationFdSet` lease count reaches zero is the
   complete old fd set closed (C.0 §10: closing the owner fd alone is
   insufficient).
3. **Executor stall.** If a helper cannot be reaped (an in-kernel host call),
   the device enters `ExecutorStalled`: it is logically withdrawn **now**
   (publication of the withdrawal), the submitting record, quarantine, aliases
   and lease are retained, and no replacement fd is opened. When the helper is
   reaped, the **same** `RecoveryId` resumes under whichever transition is then
   current per `REC-6`; no second incident is created.
4. **Fresh incarnation.** Reopen the card (new open file description, next
   `IncarnationId`), acquire DRM master, spawn a new executor, rebuild the
   device's scanout pools and their resource registrations on the new fd, and
   rediscover (connectors, properties, clock).
5. **Reinstall and qualification.** Run the 3c-i acquire reinstall for that
   device: probe, final `TEST_ONLY`, a from-scratch `ALLOW_MODESET`
   installation of the desired topology with the current DPMS projection. That
   real installation, completed with its required evidence, **is** the
   qualification. Publication happens only if the topology changed (decision 2).
6. **Outcome.** Qualified → `Ready`, admission reopens. Any failure or
   `CompletionUnknown` in steps 2–5 → `RecoveryFailed`: admission stays closed,
   outputs are logically withdrawn, quarantine retained until a proven barrier.
   No timer, DPMS change or client traffic retries it; only the boundaries of
   decision 3 and C.0 §10 create one new attempt with a fresh `RecoveryId`.

### 3.2. Arbiter interaction

- The recovery runs as the device's single `LifecycleTransition` (`REC-4`).
  A higher-priority event (`Shutdown`, `DeviceRemoved`, `VTRelease`,
  `DeviceAddedOrReplaced`, `VTAcquire`, `AdministrativeReprobe`,
  `IdentityChangingHotplug`) supersedes it; the winner absorbs the cleanup and
  receives all quarantine, and the incident's fate follows the `REC-6` matrix.
- DPMS while `Poisoned`/`Recovering` issues no KMS mutation; DPMS-on resumes a
  paused attempt but never creates one (C.0 §10 lifecycle table).
- No step may open an fd, run final `TEST_ONLY` or install state unless its
  transition id and lifecycle epoch are still current (`REC-4`); a late executor
  result for the old incarnation cannot promote state.
- While a recovery runs the X11 core stays responsive and rendering continues
  off-screen.
- **Late executor results** (`REC-4`): a result from the old incarnation that
  arrives after the recovery (or its successor) moved on cannot promote state.
  An explicit rejection permits generation-local never-submitted cleanup. An
  explicit success is accepted-stale: the owner adopts and closes every
  returned fd exactly once, terminalizes the protocol work once, and both
  possible state/resource sets go into the **current winning transition's**
  quarantine. An absent or invalid result stays acceptance-unknown with the
  same quarantine. The executor lease keeps the old incarnation from retiring
  until return or reap.

## 4. 3d-ii — quarantine, `Removed` teardown and shutdown

### 4.1. Inventory and transfer of retained owners

3d-ii inventories every resource earlier stages retained "for stage 3's
teardown barrier" and gives each an owner and a release proof:

- 2c-i resource terminalization (§3) and its teardown handoff contract, and the
  2c-i debt §4.3 retained owners;
- Cfb §3 and Ciii `CompletionUnknown` and unflip-unknown rows;
- 3c: resources of a release whose commit stayed unknown, probe-worker fds kept
  in a stuck worker (retired with their incarnation), and the `Removed`
  quarantine.

The inventory is part of the plan's first task and is checked against the code
(each retained owner is found by its production holder, not only by spec text).

**Transfer rule.** When a transition wins (supersession or terminalization),
all quarantine of the transition it replaces is transferred to it. Nothing is
released before its barrier: reap plus closing the complete `IncarnationFdSet`
for resources owned exclusively by that open file description; the device-loss
safe teardown order for Vulkan, GBM and dma-buf owners (C.0 §10: one raw fd
close is not a universal lifetime proof).

**Exit criterion.** The §6.4 matrix is total: for every state, every retained
resource has exactly one owner, checked by the end-state check (criterion A)
in every lifecycle test.

### 4.2. `Removed` teardown

A removed device is today withdrawn with its resources quarantined (3c). 3d-ii
executes its teardown in barrier order: reap the old executor/helpers, close the
complete fd set, release the resources owned by that incarnation, in the
device-loss safe order. A device that reappears is a new identity (out of scope:
opening a hot-added card).

### 4.3. Orderly shutdown

Following C.0 §10's Shutdown row:

1. Stop admission and alias creation; record `Invalidated(Shutdown)` for every
   recovery.
2. Request termination of every executor and helper; terminalize protocol work;
   **release the seat** (drop master, restore the VT) before waiting.
3. The **teardown supervisor** waits up to **3 s** for every child's wait/reap
   status; then closes the complete DRM fd set, drops file-owned quarantine, and
   tears down Vulkan, GBM and shared owners in their device-loss safe order.
   `shutdown_destroy_drawables` runs inside that order.
4. A child still unreaped at the deadline puts the device in
   `ShutdownExecutorStalled`: logical shutdown is complete, the unreaped lease is
   recorded, and the process exits; the orphaned helper keeps the device lock.

## 5. Evidence

Criteria A (end-state/leak check), B (core-loop driver only), C (hardware per
task touching a real path), F (gates and mutations under CPU load). Each test
names the mutation that must fail. The umbrella §4 differential gate applies.

### 5.1. Deterministic tests (`--lib`)

- `REC-1`: exactly one attempt per incident; a failure or unknown completion
  inside the attempt reaches `RecoveryFailed`; no recursion; timers, DPMS and
  client traffic never retry.
- Bounded administrative reprobe: one attempt per `RecoveryFailed`; the second
  forced reprobe creates none.
- `ExecutorStalled`: immediate logical withdrawal; reap resumes the same
  `RecoveryId`; no replacement fd while a lease survives.
- Supersession during recovery (shutdown, removal, VT release): the winner
  receives the quarantine; the incident follows `REC-6`.
- Topology invisibility (decision 2): across a successful recovery with an
  unchanged topology, the RANDR topology replies and notifications clients
  receive equal those of a run without the failure; the scenario includes a
  client RANDR mutation sent during the closed-admission interval, which waits
  in the gate and then executes (decision 2a), and an in-flight Present, which
  is terminalized. A second case ends in `RecoveryFailed`: the parked mutation
  is validated against the withdrawn state.
- Late executor results after supersession: an accepted-stale result's fds are
  adopted and closed exactly once and both resource sets reach the winning
  transition's quarantine; a rejection cleans up only never-submitted state.
- Quarantine totality: every inventoried owner is held and released only at its
  barrier.
- Shutdown: the complete barrier order; `ShutdownExecutorStalled` at the 3 s
  deadline with the lease recorded.

### 5.2. Hardware (card1, at least 4 cycles each)

1. **Injected completion loss:** a real commit's completion event is withheld
   until its deadline → `Poisoned` → recovery reopens card1 → `Ready`. On the
   monitor: a freeze under about one second, then normal output.
2. **Failure inside the recovery:** → `RecoveryFailed`; the server keeps
   running; the device stays withdrawn.
3. **Shutdown with a delayed helper:** → `ShutdownExecutorStalled` within 3 s,
   lease recorded.

## 6. Process

- Codex implements; the coordinator verifies every task (suites under load,
  mutations, hardware) and commits.
- **Hardware permission (user, 2026-10-04):** codex may run the 3d hardware
  tests it writes — only those named tests, with a check that the active VT is
  the coordinator's before every run, and a run cap per prompt. The coordinator
  re-runs them before each commit.
- Plans: 3d-i and 3d-ii, each at most ~14 tasks, split further if they grow.

## 7. Out of scope

- Opening a hot-added card (outside C.0).
- One `ResourceService` per Owner device (stages 4/5, activation).
- Atomic cursor and gamma, and the cursor detach in release commits (stage 4).
