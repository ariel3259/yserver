# Stage 3d-i-1 — the recovery core: `Poisoned` → fresh incarnation → `Ready`/`RecoveryFailed`

> **Implementer:** codex (model `gpt-6-luna`, reasoning effort `xhigh`; `max` from the first send-back),
> run with `< /dev/null` as a detached `systemd-run --user --unit=... -p MemoryMax=16G -p MemorySwapMax=0`
> service. Codex writes the code; this plan gives interfaces, invariants, tests and mutations, not code.
> **Hardware (user, 2026-10-04):** codex may run **only the `c0_hw_3d*` tests this plan names**, checking
> `test "$(cat /sys/class/tty/tty0/active)" = tty3` before every run (stop if not), at most 25 runs per
> prompt; the coordinator re-runs them before each commit. Every other `c0_hw_*`, `_drm` or
> `render_acceptance` test stays forbidden. No git writes.

**Revision 4 (2026-10-04, coordinator) — implementation stop on Task 1, confirmed:** the registry's
family-closure mint does not hold the last reference to the DRM device — `DeviceCleanupIo` keeps an `Rc`
clone (`drm_cleanup.rs` ~265), `KmsDevice.device` keeps one (`platform.rs` ~2544) and stays in `poll_fds` —
so `try_mint_file_family_closed` can mint a closure proof while the fd is open (a 2c-i defect: "if it
affects us, it is ours"). Task 1 now fixes the mint and detaches the device first (see Task 1, **IN-0**).

**Revision 3 (2026-10-04, coordinator) — review loop closed after round 2** (0 blocking, 1 major,
`../findings/2026-10-04-stage-3d-i-1-plan-review-round2.md`; all round-1 findings applied, M-2 partial):
no design defect remains (user's convergence rule). Folded in without another round: Task 6 runs the
recovery hardware case for **4 cycles** (M-2); the round's open integration question becomes
implementation note **IN-1** in Task 8 (the retained old executor keeps being polled and drained, with
request/transition correlation, until reaped).

**Revision 2 (2026-10-04, coordinator)** — codex round 1 (1 blocking, 2 major, 1 minor, all confirmed,
`../findings/2026-10-04-stage-3d-i-1-plan-review-round1.md`): the acquire preparation's synchronous
forced connector probe moves to a probe worker before recovery reuses it (B-1, new Task 5, which also
removes the preparation's `cfg(not(test))` fork); one authoritative route for a completion loss on the
fresh incarnation during the attempt (M-1, decision 6, Task 6); incremental hardware per real-path task
(M-2); the family-closure sequence is explicit (m-1, Task 1). Nine tasks.

**Revision 1 (2026-10-04).** 3d-i is split (pre-approved split rule, plan size drives defect density):
**3d-i-1** (this plan) is the recovery core; **3d-i-2** carries the RANDR gate during recovery (spec
§3.3), the administrative-reprobe delivery and bound (decision 3), the two integration questions of
design review round 5, the topology-invisibility differential (decision 2) and the failure-path
hardware. A test here uses only what 3d-i-1 delivers.

**Goal:** an Owner device poisoned by a completion loss leaves `Poisoned` through exactly one automatic
recovery: the old fd family is retired behind its barrier (or `ExecutorStalled` holds it), a fresh
incarnation is opened and reinstalled through the acquire's device-scoped preparation, and the device
reaches `Ready` — or `RecoveryFailed`, withdrawn — with no retry from timers, DPMS or client traffic.

**Spec:** `docs/superpowers/specs/2026-10-04-phase-c0-stage-3d-recovery-quarantine-shutdown-design.md`
revision 5 — §2 decisions 2, 5, 6; §3.1 (all); §3.2; §4.2a (lease holders, as the recovery barrier
needs them); §5.1 (`REC-1`, `ExecutorStalled`, supersession, late results, stuck probe worker); §5.2
hardware case 1. C.0 §6.4, §10 (incarnation poison, recovery, the lifecycle table), `REC-1`..`REC-6`.
Umbrella `2026-09-22-phase-c0-stage-3-lifecycle-design.md` §2 (coordinator/arbiter/driver), §3 "3d", §5.
Branch tip `861f176f`.

## Global constraints

- Production stays `Legacy` (stage 5 activates `Owner`); a **Legacy-only** server is unchanged byte for
  byte. Everything below applies to Owner devices.
- **One automatic attempt per incident (`REC-1`).** A `RecoveryId` is never derived from a
  `LifecycleTransitionId`; no timer, DPMS change or client request creates or retries an attempt.
- **No fd is opened, no final `TEST_ONLY` runs and no state is installed unless the transition id and
  lifecycle epoch are still current** (`REC-4`).
- **Barrier before reopen:** the replacement open file description is created only after every lease
  holder of the old incarnation is released — executor/helper children reaped **and** connector-probe
  workers joined (spec §4.2a) — submitters are detached, and the cleanup registry mints the file-family
  closure (`DrmCleanupRegistry::try_mint_file_family_closed`). Closing the owner fd alone is not the
  barrier.
- **A successful recovery publishes nothing** when the reinstalled topology equals the published one
  (decision 2); a failure publishes one urgent withdrawal (3c-i's).
- Standing test rules: **(A)** every lifecycle scenario ends with `c0_3bi_assert_end_state` (or its
  successor) naming its expected live set and every retained quarantine; **(B)** the backend advances
  only through the existing core-entry driver (`c0_3bi_core_driver_until*`), executor replies through
  the executor; **(C)** hardware after every task touching a real path; **(F)** each task's suites and
  every mutation are run under CPU load (16 busy loops, exact commands, default threading, counts
  reported per run).
- **No `#[cfg(test)]` fork inside a production function body.** Wall-clock deadlines get a seam field
  whose default keeps production behaviour (as `completion_deadline_override`).
- A test in task N uses only what tasks 1..N deliver.

## Design decisions this plan fixes

1. **The recovery is a lifecycle transition of kind `NormalRecovery`** owned by the device's arbiter
   (`REC-4` lowest precedence). The arbiter already emits `AllocateRecoveryIncident` on a completion
   loss (3a, `coordinator.rs` `apply`); 3d-i-1 adds the driver that executes the attempt and feeds its
   outcome back (`RecoveryOutcome::{Qualified, Failed, Stalled, Superseded}` or the existing types the
   arbiter already defines — reuse before adding).
2. **The incarnation's lease ledger is the existing `IncarnationFdSet`** (executor `mod.rs`), wired in
   production for the first time: the executor's helper lease and each probe worker's dup register on
   it; `remaining_leases() == 0` is one input of the barrier, the cleanup registry's family-closure mint
   is the other. 3c-ii's `ProbeWorkerLedger` stays the worker owner and reports into the set.
3. **Reopen uses the production open path** (`platform/drm.rs` `Device::open` + master + atomic caps),
   factored so startup and recovery share it; the new device entry gets `IncarnationId::next`, a new
   `KmsIoExecutor`, a new `DeviceCommitOwner`, a new resource service bound to the new incarnation, and
   fresh scanout pools registered on it. The old entry's objects move into the transition's quarantine;
   none is dropped before its barrier.
4. **Reinstall reuses `lifecycle_prepare_acquire_topology` and the acquire installation, device-scoped**,
   under the `NormalRecovery` transition — not the acquire episode (spec §3.3: no gate episode). The
   installation's completion with its required evidence is the qualification. **First (Task 5), the
   preparation stops probing on the core:** today it calls `output_for_exact_probe_assignment` →
   `get_connector(connector, true)` synchronously (`admission.rs` ~3272, `modeset.rs` ~720) and holds a
   `#[cfg(not(test))]` fork in its body. The forced connector discovery it needs runs on 3c-ii's probe
   worker for the incarnation being installed (lease registered, joined, result tagged with incarnation
   and transition epoch); the preparation consumes that result on the core; the transition id/epoch is
   revalidated before final `TEST_ONLY` and install. The fork is removed (the test path takes the
   production branch with a scripted prober, 3c-ii decision 4). The VT acquire benefits the same way.
5. **`ExecutorStalled`:** when a lease holder cannot be released (no wait status, worker not returned),
   the device is logically withdrawn at once (3c-i's urgent withdrawal) and the attempt waits; the later
   reap/join resumes the **same** `RecoveryId` under the transition then current (`REC-6`).
6. **One route for a loss during the attempt (M-1).** A completion loss on the fresh incarnation while
   the device is `Recovering(id)` with an `Attempting` incident is reported through the existing
   `lifecycle_report_completion_loss` path; the arbiter's loss handling, **when the state is
   `Recovering(id)`**, enters `RecoveryFailed` and resolves that incident as failed (Table U) in the same
   input — it does **not** return to `Poisoned`, and no second attempt-outcome input is delivered for
   the same terminal result. Exactly one of {completion-loss route, attempt-outcome route} reports a given
   terminal result; the plan uses the completion-loss route for unknown completion and the
   attempt-outcome route for explicit rejection or reopen/rediscovery failure.

## Review Focus

1. A second completion loss **during** the attempt (on the fresh incarnation) → `RecoveryFailed`, never
   a second attempt (`REC-1`).
2. A VT release arriving while the old family's barrier is still pending → the release wins, the
   incident follows `REC-6`, no reopen happens after the seat is gone.
3. A probe worker stuck in `GETCONNECTOR` on the old incarnation while executor children are already
   reaped → no reopen until the worker is joined (`ExecutorStalled`).
4. A late `Accepted` result from the old executor after the fresh incarnation is installed → adopted and
   closed once into the winning quarantine, never promoted onto the new incarnation.
5. DPMS-off while `Recovering` → no KMS mutation on the old incarnation; DPMS-on resumes the paused
   attempt and never starts a second one.

---

## Task 1 — the incarnation lease ledger and the old-family barrier

**Deliver:** production wiring of `IncarnationFdSet` per Owner incarnation (decision 2); the executor's
helper lease and every connector-probe worker's dup register a lease and release it on reap/join; a
`barrier_ready(incarnation)` predicate (name free) that is true only when the set has no lease, every
submitter is detached, and the cleanup registry minted the family closure. **Sequence (m-1):** (1)
release every lease holder (reap children, join workers); (2) close the control alias and discharge every
other accounted alias; (3) obtain the closure proof through `try_mint_file_family_closed`, which performs
the final close. The control close is step 2, never before step 1, and is never itself taken as the
family-closure proof.

**IN-0 (rev 4, implementation stop):** the mint must be a real last close. (a) Step 2 also detaches the
device from the platform: the `KmsDevice` entry's `Rc<drm::Device>` is moved out of the entry (the entry
itself moves to the transition's quarantine without a device handle) and the device's fd leaves
`poll_fds`. (b) `try_mint_file_family_closed` drops `DeviceCleanupIo`'s clone and then requires
`Rc::try_unwrap` on its own handle to succeed before dropping it and returning `FileFamilyClosed`; if any
other strong reference remains, it returns an error naming the outstanding holder count and mints
nothing. A closure proof therefore implies the open file description is closed. On the `Poisoned` entry the
driver stops admission and alias creation, detaches submitters/event readers, requests executor and
helper termination, and polls the barrier from `before_block`/`next_wakeup` (a wakeup armed while a
lease is outstanding).

**Interfaces — produces:** `IncarnationFdSet` reachable from the Owner device entry; the barrier
predicate; a driver hook `on_poisoned(device)` (name free) called from the existing `Poisoned` entry.

| Test | Scenario | Must fail under |
| --- | --- | --- |
| `c0_3di_poison_requests_termination_and_retires_the_family_vulkan` | completion loss → `Poisoned`; the helper is reaped and no worker is live: the barrier mints exactly once, then the old fd set is closed; the old pool has no GBM device or BO after mint | close the control alias before the lease holders are released; treat the control close alone as the closure proof; or drop only the pool's `GbmDevice` `Rc` while keeping its BOs |
| `c0_3di_stuck_probe_worker_blocks_the_barrier_vulkan` | children reaped, one probe worker blocked: the barrier stays false; joining the worker makes it true; the core has no ≤1 ms barrier poll | count only executor leases; arm an unbounded 1 ms barrier poll |
| `c0_3di_mint_refuses_while_the_device_is_still_referenced` | an extra `Rc<drm::Device>` is held (as `KmsDevice` held it before IN-0): the mint returns an error and no proof; after the holder is dropped it mints, and the fd is closed (fstat on a dup taken earlier shows the description gone, or the device's fd number is reusable) | mint without `try_unwrap` (drop only the registry's own handle) |
| `c0_3di_payload_alias_is_registered_unconditionally` | adopting a file-owned payload records one family alias | put `register_payload_alias` only inside `debug_assert!` and run this test in release mode |
| `c0_3di_native_gbm_device_outlives_file_owned_bo_discharge` | the family barrier destroys file-owned GBM BOs while retaining their native `gbm_device`, then releases the device before mint | drop only the pool's `GbmDevice` `Rc` before discharging its BOs |
| `c0_3di_unreaped_helper_blocks_the_barrier_vulkan` | the helper gives no wait status: the barrier stays false and a wakeup stays armed | treat termination request as reap |

**Hardware (M-2):** `c0_hw_3di_poison_retires_family_on_card1_drm` — on card1, a real composed commit's
completion is withheld past its deadline (deadline seam or held poller, no `cfg(test)` fork): `Poisoned`,
the helper is reaped, the closure is minted once, the device stays closed; end-state check names the
quarantine. 4 cycles (fresh fixture each).

## Task 2 — the `NormalRecovery` transition and `REC-1`

**Deliver:** the driver side of decision 1. When the arbiter allocates the incident and the barrier is
ready, the transition enters `Recovering(id)`; its outcome is fed back as one arbiter input; a failure
or `CompletionUnknown` inside the attempt enters `RecoveryFailed` with logical withdrawal (3c-i's urgent
withdrawal publication) and quarantine retained. No path re-enters `Recovering` for the same incident.
Use a scripted reopen/reinstall seam (runtime, chosen at construction — no `cfg(test)` branch) so this
task can test the state machine before Tasks 3–4 deliver the real reopen.

| Test | Scenario | Must fail under |
| --- | --- | --- |
| `c0_3di_rec1_one_attempt_per_incident_vulkan` | poisoned → attempt; the attempt's reinstall reports `CompletionUnknown` → `RecoveryFailed`; then DPMS cycles, a client modeset and many `next_wakeup`s: no second attempt, no new `RecoveryId` | re-enter `Recovering` on DPMS-on |
| `c0_3di_recovery_failed_withdraws_once_vulkan` | the attempt fails: exactly one urgent withdrawal reaches the listener; admission stays closed; the end state retains the quarantine | publish the withdrawal twice |
| `c0_3di_timer_never_retries_vulkan` | `RecoveryFailed` and the deadline wakeups fire: no attempt | arm a retry timer |

## Task 3 — the fresh incarnation: reopen, master, executor, rediscovery

**Deliver:** decision 3 — the shared open path, the new entry (`IncarnationId::next`, executor, owner,
property/connector rediscovery, clock probe), the old entry's objects moved into the transition's
quarantine. The open happens only with the barrier ready and the transition id/epoch current.

| Test | Scenario | Must fail under |
| --- | --- | --- |
| `c0_3di_reopen_waits_for_the_barrier_vulkan` | poisoned with a lease outstanding: no open is attempted until it is released | open on termination request |
| `c0_3di_fresh_incarnation_identity_vulkan` | after reopen: a new `IncarnationId`, a new executor and owner, the old ones in quarantine; a result tagged with the old incarnation is stale | reuse the old `IncarnationId` |
| `c0_3di_stale_transition_cannot_open_vulkan` | a VT release supersedes the attempt between barrier and open: no open | skip the id/epoch check before open |

**Hardware (M-2):** `c0_hw_3di_reopen_fresh_incarnation_on_card1_drm` — after Task 1's poison on card1,
the barrier releases and the device is reopened: the new fd holds DRM master, a new executor answers a
probe, rediscovery finds HDMI-2; extended in Task 4 to assert the new pools are allocated and registered
on the new incarnation. No reinstall yet (Task 6). 4 cycles.

## Task 4 — scanout pools and resource registrations on the new fd

**Deliver:** new scanout pools allocated and registered on the new incarnation's resource service;
the old pools, framebuffers and resource pair retained in quarantine until their barrier (the old
service stays serviceable for completions still owed — the full answer is 3d-i-2's integration task;
here: nothing of the old incarnation is destroyed early and nothing new is registered on the old
service).

| Test | Scenario | Must fail under |
| --- | --- | --- |
| `c0_3di_pools_rebuilt_on_the_new_incarnation_vulkan` | after reopen every managed BO's `AllocationKey` carries the new incarnation; the old keys are frozen in quarantine | register new BOs on the old service |
| `c0_3di_old_pool_survives_until_its_barrier_vulkan` | an old pool BO still `OnScreen` at poison: it is not destroyed before the family closure | destroy old pools on reopen |

## Task 5 — the acquire preparation probes off the core (B-1)

**Deliver:** decision 4's first half. `lifecycle_prepare_acquire_topology` no longer calls
`get_connector(..., true)` on the core: the forced discovery for the incarnation being installed runs on
the 3c-ii probe worker (lease on the incarnation's `IncarnationFdSet`, joined on return), its result is
tagged with incarnation and transition epoch and consumed by the preparation on the core; a stale result
is discarded. The preparation's `#[cfg(not(test))]` fork is removed; tests drive the production branch
through the scripted prober seam. The VT acquire uses the same path (its existing tests keep their
assertions).

| Test | Scenario | Must fail under |
| --- | --- | --- |
| `c0_3di_preparation_probe_is_off_the_core_vulkan` | the prober for the incarnation being installed is blocked: the core keeps serving requests and deadlines; a VT release supersedes the preparation; no install reaches the executor | call `get_connector` on the core |
| `c0_3di_stale_preparation_probe_discarded_vulkan` | the preparation's probe answers after its transition was superseded: discarded, nothing installed | skip the epoch check |
| `c0_3ci_acquire_reinstall_probe_off_core_vulkan` | the VT acquire reinstall with a blocked preparation probe: same responsiveness | keep the synchronous probe on the acquire path |

**Hardware:** `c0_hw_3c_vt_switch_on_card1_drm` is the user's `~/vt.sh` test and is **not** in codex's
permission; the coordinator runs it (or asks the user) after this task. Codex runs Task 3's hardware test
again to confirm rediscovery still works.

## Task 6 — reinstall and qualification through the acquire preparation

**Deliver:** decision 4 — `lifecycle_prepare_acquire_topology` + the acquire installation for that device
under `NormalRecovery`, with the current DPMS projection; completion with evidence → `Ready`, admission
reopens; failure/unknown → `RecoveryFailed` (Task 2's path). Topology unchanged → no publication.

| Test | Scenario | Must fail under |
| --- | --- | --- |
| `c0_3di_recovery_reaches_ready_silently_vulkan` | completion loss → recovery → `Ready`; the published RANDR state and the listener's events are unchanged; composition resumes | publish on success with an unchanged topology |
| `c0_3di_recovery_honours_dpms_off_vulkan` | poisoned while DPMS is off: the reinstall installs `ACTIVE=0`; `Ready`; DPMS-on lights through the ordinary path | install ACTIVE=1 regardless |
| `c0_3di_reinstall_failure_is_recovery_failed_vulkan` | the reinstall is rejected: `RecoveryFailed`, withdrawn, no retry | retry the reinstall once |
| `c0_3di_unknown_completion_during_attempt_vulkan` | *(M-1)* the reinstall commit on the fresh incarnation is accepted and its completion is lost (through the executor and the core driver, not the scripted seam): `RecoveryFailed` in one input, the **same** `RecoveryId`, one withdrawal, never `Poisoned`, no retry | route the loss through the generic `Poisoned` handling while `Recovering` |

**Hardware:** `c0_hw_3di_completion_loss_recovers_on_card1_drm` is delivered and run here for **at least
4 cycles** using only Tasks 1–6 (round 2, M-2); Task 9 re-runs it as the final acceptance and records the
coverage.

## Task 7 — `ExecutorStalled` and resume of the same incident

**Deliver:** decision 5.

| Test | Scenario | Must fail under |
| --- | --- | --- |
| `c0_3di_stalled_withdraws_at_once_vulkan` | the helper cannot be reaped: `ExecutorStalled`, one urgent withdrawal now, no reopen | wait for reap before withdrawing |
| `c0_3di_reap_resumes_the_same_incident_vulkan` | the helper is finally reaped: the attempt continues with the **same** `RecoveryId` and reaches `Ready`; the outputs are republished once (they were withdrawn) | allocate a new incident on reap |
| `c0_3di_stuck_worker_is_executor_stalled_vulkan` | children reaped, a probe worker stuck: `ExecutorStalled`; its join resumes the same incident | ignore workers |

## Task 8 — supersession, DPMS and late results during recovery

**Deliver:** `REC-4`/`REC-6` integration: a higher-priority event (`Shutdown`, `DeviceRemoved`,
`VTRelease`, `VTAcquire`) supersedes the attempt, the winner receives the quarantine, the incident's fate
follows `REC-6`; DPMS while `Poisoned`/`Recovering` issues no KMS mutation, DPMS-on resumes a paused
attempt; late executor results follow `REC-4` (accepted-stale fds adopted and closed once into the
winning quarantine; rejection cleans only never-submitted state).

| Test | Scenario | Must fail under |
| --- | --- | --- |
| `c0_3di_vt_release_supersedes_recovery_vulkan` | release during the barrier wait: the release wins, no reopen, the incident is invalidated per `REC-6`; the next acquire installs the fresh incarnation | reopen after the release |
| `c0_3di_removal_supersedes_recovery_vulkan` | `DeviceRemoved` during `Recovering`: `Removed`, the quarantine transferred, no further attempt | keep the attempt running |
| `c0_3di_dpms_on_resumes_paused_attempt_vulkan` | DPMS-off pauses the attempt, DPMS-on resumes it: one attempt total | start a new attempt on DPMS-on |
**IN-1 (round 2 open question):** after the fresh incarnation replaces the old entry, the retained old
`KmsIoExecutor` stays registered with the core loop (its control/reply fd in `poll_fds`, its watchdog
ticked) until it is reaped, so a buffered reply is still drained and routed with its request and
transition correlation to the winning transition's quarantine; it never reaches the new incarnation's
owner. Test: the late-`Accepted` case below must arrive through that drained transport, not be injected.

| `c0_3di_late_accepted_result_goes_to_quarantine_vulkan` | an old-incarnation `Accepted` arrives after `Ready`: its fds are adopted and closed once, both resource sets in the winning quarantine, nothing promoted | promote the late result |

## Task 9 — hardware: injected completion loss on card1, and the coverage evidence

**Deliver:** `c0_hw_3di_completion_loss_recovers_on_card1_drm`: on card1 HDMI-2, a real composed commit's
completion is withheld past its deadline (through the `completion_deadline_override` seam or by holding
the completion poller — no `cfg(test)` fork), the device enters `Poisoned`, recovers on a fresh
incarnation and composes again; **at least 4 cycles** per run; end-state check after each cycle. The
coordinator also records the monitor with the webcam (`/dev/video0`) during the run and checks the
freeze is under about one second and the image returns. Update the writer/coverage evidence for the
lifecycle `recovery` writer if one exists (cite the tests; name 3d-i-2's rows as pending).

| Test | Scenario | Must fail under |
| --- | --- | --- |
| `c0_hw_3di_completion_loss_recovers_on_card1_drm` | 4 cycles of poison → recovery → `Ready` → a composed frame reaches `HardwareComplete` on the new incarnation | skip the reopen (reuse the old fd) |

## Gate (every task)

`cargo +nightly fmt` first, then `cargo +nightly fmt --check`; `cargo clippy --workspace --all-targets --
-D warnings` (default, `--features tcp-transport`, `--features xdmcp`); `cargo test -p yserver --lib`;
**under load** (16 busy loops): 12 consecutive runs of `cargo test -p yserver --lib c0_3di_ --
--include-ignored --skip _drm --skip c0_hw` and 5 each of `c0_3cii_`, `c0_3ci_`, `c0_3bi_`, `c0_adm`;
each mutation in the task's table under load (deterministic once; load-dependent up to 40 runs; say so
plainly if one never fails); every integration file except `render_acceptance`. Report exact counts.
Grep the task's diff for `cfg(test)` inside production function bodies and report each.

## For the user

- 3d-i is split in two plans (pre-approved rule): this one ends with the recovery working on card1;
  3d-i-2 adds the RANDR gate behaviour during recovery, the bounded administrative reprobe, the two
  integration questions from the spec review and the failure-path hardware.
- `IncarnationFdSet` is wired in production for the first time here (3c-ii left it to 3d).
