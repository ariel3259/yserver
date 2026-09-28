# Stage 3c-ii — connector hotplug and device removal on the Owner

> **Implementer:** codex (model `gpt-6-luna`, reasoning effort `xhigh`; `max` from the first send-back), run with `< /dev/null` inside `systemd-run --user --scope -q -p MemoryMax=16G -p MemorySwapMax=0`. Hard rules, restated in every prompt: **no git write commands** (the coordinator verifies and commits); of the `#[ignore]` tests run only this plan's filters, each by its own command — `c0_3cii_`, `c0_3ci_`, `c0_3bi_`, `c0_3bii_`, `c0_3aii_`, `c0_3a_`, `c0_2b_add_`, `c0_conv_ciii_`, `c0_conv_cii_`, `c0_conv_cfb_`, `c0_conv_cp_`, `c0_adm`, `c0_merge_` — with `--include-ignored --skip _drm` only when the prompt records the user's GPU approval; **never** `_drm` tests, `render_acceptance`, an unfiltered `--ignored`, a VT switch, or anything that performs a modeset or takes DRM master: the hardware tests are **written, never run**, by the implementer; no deletes outside the worktree; remove temporary instrumentation before finishing; never edit `docs/status.md`. **You write the implementation and the tests**; this plan gives the interfaces, the invariants, the named tests with the scenario each must exercise, and the mutations each must catch. Execute tasks in order, one at a time; stop with the tree dirty after each task. **Do not ask for approval inside a run** — if the plan leaves a real design choice open, or something it states does not hold in the code, the kernel or C.0, stop and report it (F8); never silently substitute a test shape, never weaken an existing assertion.

**Revision 2 (2026-09-28, coordinator)** — codex round 1 (2 blocking, 3 major,
all confirmed, `../findings/2026-09-28-stage-3c-ii-plan-review-round1.md`): a
topology episode's turn is **granted** by the core only when no mutation is in
flight, and the backend stages and commits nothing before the grant (B-1,
decision 10, Task 2); the retry armed after a timed-out probe waits for the
stuck worker to be joined (B-2, decision 6); `ENODEV` is consumed on receipt,
independent of the other devices (M-1, decision 11); a VT release invalidates
every outstanding probe, whatever its cause (M-2, decision 12); hardware after
every task on a real path, Tasks 1, 4 and 6 included (M-3).

**Revision 1 (2026-09-28, coordinator).**

**Goal:** on a server with an Owner device, every connector probe runs off the
core thread; a connector hotplug is a per-device lifecycle transaction
published once per episode; a removed Owner device is withdrawn at once and
the server continues.

**Spec:** `docs/superpowers/specs/2026-09-26-phase-c0-stage-3c-vt-and-hotplug-design.md`
revision 5 — §4 (all), §3.2 (removal while released), §3.3 (the acquire probe
moves off the core thread), §5 exceptions 2 and 4, §6 (the 3c-ii half, plus
the two 3c-i tests carried here). Umbrella:
`2026-09-22-phase-c0-stage-3-lifecycle-design.md` §3 "3c", §4, §5. C.0 §10
(device-removal row, `ExecutorStalled`, `CompletionUnknown` continuation) and
`REC-4`. Built on 3c-i as accepted at `388a019e`
(`../findings/2026-09-27-stage-3c-i-accepted.md`): the urgent withdrawal, the
episode turn (`EpisodeBegin`/`EpisodeEnd`), `AcquireEpisode`, the per-device
acquire probe.

## Global constraints

- Production stays `Legacy`. A **Legacy-only** server keeps today's
  synchronous `run_display_rescan`, `reprobe_connectors`,
  `on_display_hotplug` and `on_vt_acquire_legacy` byte for byte (a test pins
  each). Everything below applies to a server with at least one Owner device.
- The client contract is parity with Legacy; the only differences are the
  spec's §5 exceptions (2 and 4 here; 1, 3, 5 are 3c-i's).
- **Probe deadline: 2 s** (`PROBE_EPISODE_DEADLINE`), for every probe episode
  (udev edge, forced reprobe, acquire). Task 7 measures the probe on card1 and
  records it; the constant changes only if the measurement exceeds it (then
  F8, not a silent raise).
- Hotplug debounce stays **150 ms** (today's).
- Every scenario test obeys the standing rules: **(A)** it ends with
  `c0_3bi_assert_end_state` (or its successor) stating its expected live set
  explicitly; **(B)** the backend advances only through the existing
  core-entry driver (`c0_3bi_core_driver_until`,
  `c0_3bi_core_driver_until_with_state` for anything asserting client bytes,
  replies or publications) and executor replies go through the executor
  (`StubBehaviour::AcceptKernelCalls` or scripted replies), never injected into
  the Owner alone — a stub that cannot reproduce a kernel side effect says
  which in a comment; **(C)** the coordinator runs hardware after every task
  that touches a real path. No new harness (plan 3b-i-2 rev 4).
- **No `#[cfg(test)]` fork on a real path** (3c-i finding 1: the hardware test
  is compiled with `cfg(test)` and ran the test branch). Scripted probe
  results enter through a runtime seam chosen at construction (below), never
  through a `cfg(test)` branch inside a function the hardware test also runs.

## Design decisions this plan fixes

1. Test names start with `c0_3cii_` (the two carried 3c-i tests keep their
   `c0_3ci_` names); Vulkan tests end in `_vulkan` and use the 3b/3c-i fixtures
   (`owner_live_fixture*`, the two-Owner position fixture, the mixed
   Legacy+Owner fixture of 3b-i-2 Task 4). Mutations are numbered **H1…**.
2. **The worker's fd is a `dup` of the device's master fd, not a fresh open of
   the node.** *(Correction to spec §4.1, whose parenthetical says no master is
   needed.)* The kernel performs the forced probe (`fill_modes`: EDID re-read,
   fresh mode list) only when the calling `drm_file` is the current master;
   otherwise it logs "demoting to read-only probe" and returns cached state
   (`~/Projects/linux/drivers/gpu/drm/drm_connector.c:3376-3386`,
   `drm_mode_getconnector`). A `dup` (`F_DUPFD_CLOEXEC`) shares the open file
   description, hence the `drm_file`, hence master. Probes never run while the
   seat is released (edges are recorded, spec §3.2), and the acquire probe runs
   after `drmSetMaster`, so the dup'd fd is master whenever a worker uses it.
3. **Fd tracking.** Spec §4.1 says the worker's fd is "registered in the device
   incarnation's fd set (C.0's tracked fd ownership)". `IncarnationFdSet`
   (`kms/executor/mod.rs`) has **no production instance** today (it is
   `#[allow(dead_code)]`, used only by its own unit tests). This plan does not
   wire it; it adds a per-device **`ProbeWorkerLedger`** in the backend that
   owns each worker's `JoinHandle` and its fd lifetime: the fd moves into the
   worker thread and is closed when the worker's call returns; a worker that
   missed its deadline stays in the ledger as **stuck** until it returns, then
   it is joined. The ledger is retired with the device (removal keeps a stuck
   entry until it returns; 3d owns incarnation teardown). Wiring the ledger
   into `IncarnationFdSet` is 3d's.
4. **The probe seam.** A `ConnectorProber` (trait object or enum, the
   implementer's choice) is chosen when the backend is built: production
   performs the real ioctls on the worker's fd; fixtures install a scripted
   prober whose per-device answers are `Ok(snapshot)`, `Err(errno)` —
   including `ENODEV` — or **block on a barrier the test controls** (a real
   worker thread really blocked, not a scripted late reply). The worker runs
   the same probe function the Legacy site it replaces runs:
   `probe_connector_snapshots` for the udev edge and the acquire,
   `probe_connectors` for the forced reprobe (Legacy's `reprobe_connectors`
   uses the lighter probe, and parity needs the same data).
5. **Result delivery.** A worker sends its tagged result
   (`DrmDeviceKey`, incarnation, probe epoch, the result) on a channel the
   backend owns and wakes the core through the existing `CoreSender`
   (`Message::CrtcConfigReady`, the backend's worker wake today). The backend
   consumes results at its state-carrying periodic entry, where the debounced
   rescan fires today (`poll_deferred_input`); the episode deadline is part of
   `next_wakeup`. If `poll_deferred_input` does not run on the iteration after
   such a wake, stop and report (F8) — do not add a polling timer.
6. **One probe episode at a time** per server. An edge arriving while a probe
   episode or a topology episode is open is recorded and re-arms the debounced
   rescan when that episode ends; it never starts a second concurrent
   episode. (Legacy rescans are synchronous and never overlap; this keeps that.)
   *(Rev 2, B-2.)* **A pending retry outlives a stuck worker.** A rescan armed
   while a device has a stuck worker (the background rescan after a
   forced-reprobe expiry, or an edge that found a device stuck) is not spent on
   an episode that would fail that device at once: it stays pending and starts
   a fresh probe episode when the stuck worker returns and is joined. A retry
   is consumed only by an episode in which every device had a worker it could
   run.
7. **A lifecycle transition only when there is KMS work.** Classification
   (§4.2) is computed for every changed device. A device whose change needs a
   KMS commit — an active output whose connector is gone, or a remembered
   route to relight — projects `IdentityChangingHotplug` or `TopologyRebuild`
   into its arbiter and runs one transaction (§4.3). A device whose change is
   logical only (a connector appeared with no remembered route, an inactive
   connector left, a mode list changed on an inactive connector) projects no
   transition: its registry change is staged into the episode and the
   participant is terminal at once, as Legacy publishes the same change
   without a modeset. No empty atomic commit is ever sent.
8. **The forced reprobe changes no KMS state** (Legacy parity:
   `reprobe_connectors` → `publish_connector_probes` never modesets and never
   disables an enabled output; its `Backend` doc says so). On Owner its probe
   moves off the core thread and its result is published in its gate turn;
   its classification is `AdministrativeReprobe`, recorded, and **projects no
   lifecycle transition**. A physically vanished active output found by a
   forced reprobe is left to the udev edge's episode, as on Legacy.
9. **Hotplug kinds never reach the DPMS-shaped description.** Today an
   arbiter transition of a hotplug kind would fall into
   `lifecycle_topology_description` (`render/admission.rs`, the `ACTIVE`-only
   DPMS/VT-release shape) through `admission_dispatch_lifecycle_topology`.
   Task 2 gives `IdentityChangingHotplug` and `TopologyRebuild` their own
   prepared description; no task before Task 2 projects either kind.
10. *(Rev 2, B-1.)* **A topology episode's turn is granted, not taken.**
    Today the core's `begin_topology_episode` sets the episode as soon as it
    drains `EpisodeBegin`, even while a client mutation is in flight, and
    `EpisodeEnd` publishes regardless (`core_loop/run.rs`). From Task 2, an
    `EpisodeBegin` is a request: the core **grants** it when no mutation is in
    flight (queued behind the in-flight one otherwise, ahead of later waiters)
    and tells the backend through a new `Backend` notification carrying the
    episode id; the backend **stages no logical change and dispatches no
    commit** for any participant before the grant. 3c-i's acquire episode keeps
    working unchanged: its `EpisodeBegin` is consumed before pending requests
    are drained, with the seat released no client mutation can be in flight,
    so it is granted at once (a test pins this). An urgent withdrawal still
    bypasses both.
11. *(Rev 2, M-1.)* **`ENODEV` is consumed on receipt.** A probe result
    answering `ENODEV` raises that device's `DeviceRemoved` when the core
    consumes it, not when the episode resolves: its urgent withdrawal does not
    wait for the other devices' answers or the deadline, and the other devices'
    outcome (complete, failed, late) cannot hide it. The device leaves the
    episode.
12. *(Rev 2, M-2.)* **A VT release invalidates every outstanding probe.** At
    the release's prompt obligations, any open probe episode — hotplug,
    forced reprobe, acquire — has its epoch invalidated (late answers are
    discarded; a stuck worker stays in the ledger); a hotplug episode that had
    not yet been granted ends without staging anything and its edge is
    recorded for the acquire (spec §3.2); a granted hotplug episode whose
    transactions are dispatched follows 3c-i's release rules for dispatched
    work; a parked forced reprobe resolves `Expired` (the reply carries the
    published state, the turn is released) and **no background rescan is
    armed** while released — the acquire probe covers it.

## Review Focus

1. HDMI unplugged while a client `SetCrtcConfig` on the same device is
   dispatched and unanswered (Task 2
   `c0_3cii_hotplug_waits_for_dispatched_client_modeset_vulkan`).
2. A monitor whose EDID read hangs in the kernel: the worker is stuck, the
   next edge must fail that device at once and the server keeps serving
   clients (Task 1 `c0_3cii_stuck_worker_fails_next_probe_fast`).
3. Unplug and replug inside the 150 ms debounce, or a replug while the unplug's
   episode is still open (Task 2 `c0_3cii_edge_during_open_episode_rearms_vulkan`).
4. A GPU unbound while the VT is away, then the user switches back (Task 5
   `c0_3ci_acquire_skips_a_removed_device_vulkan`; Task 4
   `c0_3cii_removal_while_released_is_not_deferred_vulkan`).
5. A desktop that polls `GetScreenResources` every second while a monitor is
   plugged in (Task 6 `c0_3cii_forced_reprobe_parks_and_expires`, and the
   reprobe never modesets: `c0_3cii_forced_reprobe_never_commits_vulkan`).

---

## Task 1 — the probe worker, the probe episode and classification (spec §4.1 worker rules, §4.2)

**Deliver — the worker:** per probe, a fresh worker thread per device with a
`dup` of the device's master fd (decision 2), recorded in the device's
`ProbeWorkerLedger` (decision 3), running the installed `ConnectorProber`
(decision 4), returning a result tagged with `DrmDeviceKey`, incarnation and
probe epoch (decision 5). While a device has a **stuck** worker, a new probe of
that device fails **at once** with no second thread.

**Deliver — the probe episode:** one episode covers every open device that is
not closed or removed; it has a probe epoch and an absolute deadline (start +
`PROBE_EPISODE_DEADLINE`) armed in `next_wakeup`. A result is **stale** — and
discarded without effect — when its epoch is not the open episode's or its
incarnation is not the device's current one. The episode resolves when every
device has answered or at the deadline; a device with no answer at the
deadline counts as failed, and the epoch is invalidated so its late answer is
discarded. Outcome: `Complete(per-device snapshots)`; `Failed` if any device
failed (Legacy's combined boundary — nothing is applied); a device answering
`ENODEV` is not a failure: it is reported **on receipt** as a `Removed(device)`
event (decision 11) and leaves the episode, which resolves over the remaining
devices. *Interim until Task 4:* the consumer of that event logs it and keeps
the device out of the episode's outcome; nothing is withdrawn yet (Task 4).

**Deliver — retries and the release** (decisions 6 and 12): a rescan armed
while a device has a stuck worker stays pending until that worker is joined;
a VT release invalidates the open probe episode's epoch and records the edge
for the acquire.

**Deliver — classification:** per device, the complete result is compared with
the device's discovered topology (connector set by name, connection state,
EDID identity, mode lists): connector set or EDID changed →
`IdentityChangingHotplug`; only mode lists or other discovered objects changed →
`TopologyRebuild`; unchanged → none. A forced-reprobe episode classifies as
`AdministrativeReprobe` for a changed device (decision 8). `desired.rs`'s
`TopologyChangeClass` is the vocabulary.

**Deliver — the consumer (interim until Task 2):** on a server with an Owner
device, the debounced udev rescan starts a probe episode instead of
`run_display_rescan`'s synchronous probe; at resolution it classifies each
device and logs one line per changed device and nothing else (no registry
change, no transition, no publication) — Task 2 replaces this with the
application. A Legacy-only server keeps `run_display_rescan` unchanged.

| Test | Scenario | Must fail under |
| --- | --- | --- |
| `c0_3cii_probe_runs_off_the_core_thread_vulkan` | an Owner fixture, the scripted prober blocked on a barrier; the debounced rescan fires through the driver: the core entry returns while the worker is blocked, and a `GetScreenResourcesCurrent` from an installed client is answered from the published state before the barrier opens | **H1** run the probe inline in `poll_deferred_input` |
| `c0_3cii_probe_worker_shares_the_master_file` | the fd handed to the prober shares the open file description of the device's master fd (proved by `kcmp(KCMP_FILE)`, or a file-status flag set on one fd visible on the other) | **H2** open the device node afresh for the worker |
| `c0_3cii_stale_probe_discarded_vulkan` | two episodes back to back; the first episode's answer arrives after the second began, and a result tagged with an older incarnation arrives: neither changes the episode's collected results or its outcome | **H3** accept a result without checking epoch and incarnation |
| `c0_3cii_probe_episode_deadline_vulkan` | one device never answers: at the deadline (through `next_wakeup`) the episode resolves `Failed`; releasing the barrier afterwards delivers a late answer that changes nothing | **H4** leave the deadline unarmed / apply the late answer |
| `c0_3cii_stuck_worker_fails_next_probe_fast` | a real worker blocked past its deadline; a new episode starts: that device fails at once, no second thread is spawned for it (ledger count), the other device still probes; the barrier opens: the stuck worker returns, is joined and its fd closed (the ledger is empty) | **H5** spawn a second worker for a device with a stuck one |
| `c0_3cii_enodev_is_removal_not_failure` | two devices, one answers `ENODEV` while the other is still blocked: the `Removed` event is observed before the other answers, and the episode then resolves `Complete` with the other's snapshot; an `EIO` answer is a failure, never `Removed` | **H6** classify every probe error as removal; **H6b** report `ENODEV` only at episode resolution |
| `c0_3cii_retry_waits_for_stuck_worker` | a device's worker is stuck; an edge arms a rescan: no episode is started while the worker is stuck; the barrier opens, the worker is joined, and exactly one fresh episode runs and applies the newest answer | **H51** start the retry at once and let it fail on the stuck device |
| `c0_3cii_release_invalidates_outstanding_probe_vulkan` | a hotplug probe is outstanding; `on_vt_release`: the episode's epoch is invalidated, the late answer is discarded, the edge is recorded for the acquire, no worker is spawned while released | **H52** keep the probe episode alive across a release |
| `c0_3cii_classification` | unit, the table above: connector added, connector removed, same name with a new EDID → `IdentityChangingHotplug`; only a mode list changed → `TopologyRebuild`; forced reprobe with a change → `AdministrativeReprobe`; identical → none | **H7** classify an EDID change as `TopologyRebuild` |
| `c0_3cii_legacy_only_rescan_unchanged` | Legacy-only server: the debounced edge runs `run_display_rescan` synchronously with today's call sequence (recorded seam); no worker is spawned | **H8** route a Legacy-only server through the probe worker |

**Hardware (rev 2, M-3):** this task writes **`c0_hw_3cii_probe_worker_on_card1_drm`**
(written, never run by the implementer): opens card1 as Owner with the
production prober, runs one probe episode through the real worker (a `dup` of
the master fd, decision 2), and asserts its snapshot equals the synchronous
`probe_connector_snapshots` on the same fd, that the worker was joined, the
ledger is empty and the dup'd fd is closed; it records the probe's duration. No
cable, no VT; the coordinator runs it × 3 after this task.

## Task 2 — the Owner hotplug transaction and its episode (spec §4.3, §4.1 turn and outcome table, §4.4)

Co-delivered: the Owner hotplug route switches on only with its transaction,
its episode and its publication, so no tree exposes a half route.

**Deliver — the logical change, staged:** at a `Complete` probe outcome the
backend computes, per device, the same logical result Legacy's rescan computes
(apply the snapshot, reconcile the registry, relight requests for returned
remembered routes, layout policy with reserved slots and extent — the existing
shared logical code) **as a staged change** that the backend model adopts only
at that participant's terminal result. On a server with Legacy devices, the
Legacy participants run today's rescan steps **scoped** to them (`_for_devices`
helpers) synchronously inside the episode's turn, and **stage** their RANDR
difference into the episode instead of publishing it (3c-i's scoped resume
pattern).

**Deliver — the transaction, per Owner device with KMS work** (decision 7):
project `IdentityChangingHotplug` or `TopologyRebuild` (Task 1's classification)
through `project_device_intent`; at `PhysicalAdvanceAllowed` the admission
dispatch builds a **hotplug description** (decision 9): every active output
whose connector is gone is disabled (its CRTC off, connector detached), every
relit remembered route is enabled with its mode, CRTC and a composed primary
from a freshly prepared pool — 3b's execution (preparation, infallible
promotion, a retired bundle for each displaced pool waiting for its KMS proof)
— and **kept outputs contribute no object** to the commit. The DPMS projection
is refreshed first (3a): under protocol DPMS off, a relit output is installed
with `ACTIVE=0`. A root-storage change gives every kept output full damage and
an ordinary composed repaint, never a lifecycle object.

**Deliver — the episode:** 3c-i's `AcquireEpisode` becomes one topology
episode type with a cause (`Acquire`, `Hotplug`; Task 6 adds `ForcedReprobe`);
one open at a time. A hotplug episode signals `EpisodeBegin` when its probe
outcome is `Complete` with at least one changed participant, and holds the
turn until every participant is terminal. A participant becomes terminal on:
its transaction's `Applied` (the staged change is adopted); **rejected with
known completion** (its previous installed topology is kept, except outputs
whose connector is physically gone, which are withdrawn logically); unknown
(`CompletionUnknown`/stalled: the existing `ExecutorStalled` closure and urgent
withdrawal); logical-only (at once); a Legacy participant when its scoped steps
return. Then one `EpisodeEnd(id, publication)` built from the backend model —
`None` when nothing published changed. A `Failed` probe outcome starts no
episode and publishes nothing. *(Rev 2, B-1.)* **Nothing is staged or
dispatched before the core grants the turn** (decision 10), so a client
modeset in flight on **any** device — the changed one or another — is terminal
and published before the episode stages its change; the episode's publication
is built from the model after it.

**Deliver — the grant (yserver-core + backend)** *(rev 2, B-1)*: the core
treats `EpisodeBegin` as a request for the gate turn — granted at once when no
mutation is in flight, otherwise queued behind the in-flight one and ahead of
later waiters — and notifies the backend of the grant through a new `Backend`
entry (default no-op for other backends). An `EpisodeEnd(id, None)` for an
episode not yet granted withdraws the request. 3c-i's acquire episode is
granted at once (no mutation can be in flight with the seat released) and its
tests stay green unchanged.

**Deliver — the hardware test:** **`c0_hw_3c_hotplug_on_card1_drm`** (written,
never run by the implementer): opens card1 as Owner (it skips with a logged
reason if another process holds master), composes on HDMI-2, then × 4 prompts
the user on stderr to unplug HDMI-2 and waits (generous timeout, e.g. 60 s)
for the real udev edge → probe episode → transaction `Applied` → publication
without HDMI-2 and its pool retired; then prompts to replug and waits for the
relight `Applied`, a composed frame on HDMI-2 and a publication with it. It
records each probe's duration (Task 7 reads it). Each cycle ends with the end
state check. The coordinator runs it × 3 after this task, with the user at the
machine.

| Test | Scenario | Must fail under |
| --- | --- | --- |
| `c0_3cii_unplug_retires_and_publishes_vulkan` | two lit outputs, one connector vanishes from the scripted probe: one commit disables its CRTC, its pool is retired into a bundle that is discharged by the KMS proof; one publication without it; end state clean | **H9** destroy the pool without waiting for its proof |
| `c0_3cii_replug_relights_remembered_route_vulkan` | after an unplug, the connector returns with a compatible mode: one commit relights it with a fresh pool; a composed frame reaches it; one publication with it back at its remembered position | **H10** relight through the Legacy `enable_connector` path |
| `c0_3cii_kept_output_is_not_a_lifecycle_object_vulkan` | an unplug beside a kept lit output whose layout position changes: the commit names no object of the kept CRTC; the kept output gets full damage and an ordinary composed repaint | **H11** include the kept CRTC in the hotplug commit |
| `c0_3cii_hotplug_uses_its_own_description_vulkan` | a hotplug transition is dispatched: the committed description disables/enables connectors and planes, not the `ACTIVE`-only DPMS shape | **H12** build hotplug commits with `lifecycle_topology_description` |
| `c0_3cii_relight_under_dpms_off_vulkan` | protocol DPMS off, the remembered connector returns: installed with `ACTIVE=0`, no frame admitted | **H13** relight lit regardless of DPMS |
| `c0_3cii_logical_only_change_needs_no_commit_vulkan` | a connector appears with no remembered route: no transition, no executor send; one publication with the new connected output | **H14** send a commit (or an empty one) for a logical-only change |
| `c0_3cii_episode_publishes_once_after_every_commit_vulkan` | two Owner devices both change; A `Applied`, B held in flight: no RANDR bytes to clients and a client `SetCrtcConfig` stays queued; B terminal: exactly one publication, then the queued request proceeds | **H15** publish when the first participant applies |
| `c0_3cii_episode_partial_commit_failure_vulkan` | two Owner devices: A's commit rejected with known completion (a connector gone on A), B's commit unknown: A shows its previous topology minus the gone output, B is withdrawn (`ExecutorStalled`, one urgent withdrawal), the episode publishes once, per the outcome table | **H16** publish A's staged change after its rejection |
| `c0_3cii_episode_mixed_legacy_owner_vulkan` | Legacy + Owner, both change: the Legacy steps touch only the Legacy device and stage their difference; the Owner transaction runs; one publication carries both | **H17** let the scoped Legacy steps publish directly |
| `c0_3cii_one_failed_probe_applies_nothing_vulkan` | two devices, one probe fails (and, second case, one is late past the deadline): no staged change, no transition, no publication; the next edge starts a fresh episode that applies both | **H18** apply the devices that answered |
| `c0_3cii_hotplug_waits_for_dispatched_client_modeset_vulkan` | a client modeset dispatched on A and unanswered; A's connector vanishes: A's transaction is not dispatched until the modeset is terminal; the modeset's reply and publication come first; the episode's publication is built from the model after it | **H19** stage the hotplug change against the pre-modeset model |
| `c0_3cii_hotplug_on_b_waits_for_modeset_on_a_vulkan` | *(rev 2, B-1)* a client modeset dispatched on A and unanswered; B's connector vanishes: B's change is not staged and no B commit is sent until A's modeset is terminal and published; then one episode publication for B | **H53** stage/dispatch before the core's grant |
| `c0_3cii_core_episode_begin_waits_for_in_flight_mutation` (yserver-core) | a mutation is in flight when `EpisodeBegin` arrives: no grant is delivered until the mutation finishes; a later client mutation queues behind the episode; `EpisodeEnd(None)` before the grant withdraws the request and the queue proceeds | **H54** grant `EpisodeBegin` while a mutation is in flight |
| `c0_3cii_edge_during_open_episode_rearms_vulkan` | a second edge arrives while the episode is open: no second episode starts; after `EpisodeEnd` the debounced rescan re-arms and its episode sees the newest state | **H20** start a concurrent probe episode |

## Task 3 — publication parity: timestamps and the unplug/replug differential (spec §4.4)

**Deliver:** evidence that the Owner publication equals Legacy's for the same
change; any difference found is fixed in this task (the spec's §5 lists no
exception for hotplug publications). `lastSetTime` is preserved;
`lastConfigTime` advances when the published configuration changed, including
a partial outcome or a withdrawal; the notifications are those Legacy's
`fire_randr_changes` emits for the same resulting difference.

| Test | Scenario | Must fail under |
| --- | --- | --- |
| `c0_3cii_hotplug_timestamps_match_legacy_vulkan` | the same unplug, then replug, on a Legacy fixture and an Owner fixture: `lastSetTime` unchanged on both, `lastConfigTime` advanced on both; a metadata-only change and a partial outcome also compared | **H21** advance `lastSetTime` on a hotplug publication |
| `c0_3cii_unplug_replug_differential_vulkan` | Legacy vs Owner, unplug then replug: the event bytes each installed client receives and the `GetScreenResources`/`GetOutputInfo`/`GetCrtcInfo` replies are equal; backend state (outputs, layout, extent, registry) equal | **H22** emit the notifications before the state update |

## Task 4 — typed udev events, `DeviceRemoved` and `DeviceAddedOrReplaced` (spec §4.5, §4.6, §3.2)

**Deliver — typed monitor:** `DrmHotplugMonitor::drain` yields, per uevent, a
typed record: action (`add`/`remove`/`change`), `dev_t`, devnode, whether it is
a card node, a connector sub-device, and whether it carries `HOTPLUG=1`; the
connector-edge boolean is derived from the records. A **classifier** maps each
record to the open devices by `dev_t` → `DrmDeviceKey` (`DrmDeviceKey` is
`major:minor`) and the device's current incarnation: `remove` of an open card
node → `DeviceRemoved`; `add` of a card node that is not open →
`DeviceAddedOrReplaced`; a `change` or a connector sub-device event of an open
card → a connector edge (the debounced rescan). A Legacy-only server keeps its
behaviour (the edge boolean) unchanged.

**Deliver — `DeviceRemoved` on an Owner device**, raised only by a classified
`remove` or a probe answering `ENODEV` (Task 1's `Removed`, whose interim
failure treatment this task replaces): projected through the coordinator
(`DesiredIntent::DevicePresence { present: false, .. }`); **no KMS call** is
attempted on that device; alias creation stops; executor/helper termination is
requested; `Invalidated(DeviceRemoved)` is recorded; its outputs are withdrawn
by **one urgent withdrawal at once** — never delayed by an unreaped helper
(which stays `ExecutorStalled` holding its handles and quarantine until reap),
by an open episode (the device leaves it; the episode publishes only its
remaining participants), by a mutation holding the gate, or by a released seat;
every in-flight commit on it terminates `CompletionUnknown` into quarantine and
a client modeset token on it resolves `Failed` through the ordinary ready-token
path (3c-i); its probe ledger is retired (a stuck worker stays until it
returns); the arbiter reaches `Removed`; the server continues. **The renderer's
device** (the KMS device the selected Vulkan renderer runs on, or the
renderer's render node): the server exits, as on device loss today. A Legacy
device's removal is unchanged.

**Deliver — `DeviceAddedOrReplaced`:** one log line; the card is not opened. A
removed device whose node reappears stays `Removed`.

| Test | Scenario | Must fail under |
| --- | --- | --- |
| `c0_3cii_udev_events_classified` | typed records as the monitor produces them: remove of the open card → `DeviceRemoved`; add of an unopened card → `DeviceAddedOrReplaced`; `change` with `HOTPLUG=1` and a connector sub-device event of the open card → edge; events for other subsystems' or unopened devices' `change` → nothing | **H23** raise `DeviceRemoved` for a `remove` whose `dev_t` is not open |
| `c0_3cii_device_removed_withdraws_and_continues_vulkan` | two Owner devices, B removed (classified event through the hotplug entry): no executor send on B afterwards, termination requested, one urgent withdrawal naming B's outputs and CRTCs, B `Removed`; A keeps composing and no exit is requested; end state clean except B's quarantine, named | **H24** issue a KMS call (e.g. an `ACTIVE=0` commit) on the removed device; **H24b** `request_exit` for a non-renderer removal |
| `c0_3cii_removal_via_enodev_vulkan` | a probe episode where B answers `ENODEV` while A's worker is still blocked: B's urgent withdrawal reaches the listener **before** A answers; then A answers and the episode applies A's change without B; second case, A fails: B is still withdrawn and A's failure applies nothing else | **H25** keep treating `ENODEV` as an episode failure; **H25b** withdraw B only when the episode resolves |
| `c0_3cii_removal_while_released_is_not_deferred_vulkan` | the seat is released (3c-i release completed); B removed: withdrawn and published at once, not `Deferred(SeatReleased)` | **H26** defer the removal while released |
| `c0_3cii_removal_during_unrelated_modeset_publishes_at_once_vulkan` | a client modeset on A holds the gate (dispatched, unanswered); B removed: the withdrawal's notifications reach the listener before the modeset's reply | **H27** queue the withdrawal behind the held gate |
| `c0_3cii_urgent_withdrawal_hides_unpublished_modeset_vulkan` | A's modeset is promoted in the backend but its publication has not run; B removed; a client reads `GetScreenResourcesCurrent` in between: it sees B gone and A's old configuration; after A's publication, both changes | **H28** build the withdrawal from the backend model |
| `c0_3cii_later_publication_does_not_restore_withdrawn_outputs_vulkan` | after B's withdrawal, A's queued publication and a later hotplug episode's publication run: B's outputs stay absent and no notification re-adds them | **H29** skip the withdrawn-id filter for backend-built publications |
| `c0_3cii_unknown_release_withdrawal_during_unrelated_modeset_vulkan` | through `on_vt_release`: A's release commit never answers while a client modeset on B holds the gate; at the deadline A's withdrawal is published without waiting for B's turn | **H30** publish an unknown-release withdrawal through the ordinary FIFO |
| `c0_3cii_removal_interrupts_episode_vulkan` | an open hotplug episode over A and B; B removed mid-episode: B leaves the episode, A's transaction completes, the episode publishes only A's change | **H31** keep waiting for the removed participant |
| `c0_3cii_renderer_removal_exits_vulkan` | the removed device is the renderer's: `request_exit` | **H32** keep running without the renderer |
| `c0_3cii_device_added_is_ignored` | an unopened card is added, and a removed card reappears: one log line each, no device opened, the removed one stays `Removed` | **H33** open the new card |
| `c0_3cii_probe_error_is_not_removal_vulkan` | a probe answering `EIO`, and a udev `change` for the open card: neither raises `DeviceRemoved` | **H34** raise `DeviceRemoved` from a probe failure other than `ENODEV` |

**Hardware (rev 2, M-3; moved here from Task 7):** `c0_hw_3c_hotplug_on_card1_drm`
now also asserts the real monitor's typed records — each physical unplug and
replug of HDMI-2 yields a `change` record with card1's `dev_t` and
`HOTPLUG=1`, classified as a connector edge, and no `DeviceRemoved` — as
**`c0_3cii_real_monitor_delivers_typed_events`** (**H50** derive the edge from
the action string alone, dropping `dev_t`). A fixture cannot prove the
kernel's delivery, and this box's systemd-261 libudev drops unicast from an
untrusted sender, so no unprivileged injection exists. The coordinator runs it
× 3 after this task, with the user at the machine.

## Task 5 — the acquire probe off the core thread (spec §3.3 with §4.1)

**Deliver:** on a server with an Owner device, `on_vt_acquire` keeps its order
up to master (the `AcquireEpisode` begins first and the core reserves its turn
before draining requests; `VT_ACKACQ`; bounded `drmSetMaster` over the devices
still present — a removed device is neither asked for master nor probed), then
starts a probe episode over the present devices and **returns**. The rest of
3c-i's acquire — scoped Legacy resume, the per-participant dispositions of
spec §3.3's table, `VtState::Active`, input resume, xkb resync, the reinstalls
— runs as the **continuation** when the probe episode resolves. The
acquire episode is §3.3's exception to all-or-nothing: each participant gets
its own disposition — Legacy failure or no answer keeps today's exit; a
healthy Owner device with an answer reinstalls; an Owner device that failed or
did not answer by the deadline is closed and urgently withdrawn; `ENODEV` →
`DeviceRemoved` (Task 4). While the probe is outstanding `VtState` stays
`Resuming`, input stays paused, clients are served from the published state. A
release arriving before the continuation supersedes it: the probe epoch is
invalidated, the episode ends with `EpisodeEnd(id, None)`, nothing is
reinstalled, the hand-off follows 3c-i. The synchronous
`probe_connector_snapshots_per_device` is no longer called on this path; 3c-i's
tests that scripted it move to the Task 1 seam with their assertions intact.
Hotplug edges recorded while released are covered by this probe and the
reinstall (no separate rescan after acquire unless a new edge arrives).

| Test | Scenario | Must fail under |
| --- | --- | --- |
| `c0_3cii_acquire_probe_is_off_the_core_thread_vulkan` | the prober blocked: `on_vt_acquire` returns; input is still paused and `VtState` is `Resuming`; a client query is answered from the published state; the barrier opens: the continuation runs and input resumes | **H35** probe synchronously in `on_vt_acquire` |
| `c0_3ci_acquire_probe_no_reply_withdraws_owner_device_vulkan` | two Owner devices, B never answers: at the probe deadline A reinstalls and composes, B is closed and urgently withdrawn, the episode publishes A only; the server continues | **H36** end the server (or wait forever) when an Owner device does not answer |
| `c0_3ci_acquire_skips_a_removed_device_vulkan` | B removed while released (Task 4): at acquire B gets no `drmSetMaster` and no probe, does not trigger the exit, stays withdrawn; A reinstalls | **H37** ask a removed device for master |
| `c0_3cii_hotplug_while_released_waits_for_acquire_vulkan` | while released, A's connector vanishes (edge recorded, no probe, no worker spawned); at acquire the probe sees it, the reinstall installs the remaining outputs and one publication shows the change | **H38** probe while the seat is released |
| `c0_3cii_release_during_acquire_probe_vulkan` | acquire, then release before the probe answers: no reinstall reaches the executor, the episode aborts without publishing, hand-off as 3c-i; the late probe answer changes nothing | **H39** run the continuation after a superseding release |
| `c0_3cii_legacy_only_acquire_unchanged` | Legacy-only server: `on_vt_acquire` runs today's synchronous route (recorded seam) | **H40** route a Legacy-only acquire through the worker |

**Hardware:** the coordinator runs `c0_hw_3c_vt_switch_on_card1_drm` × 3 from a
tty (`sh ~/vt.sh`, tty4) after this task — the acquire's probe now runs on the
worker.

## Task 6 — the forced reprobe off the core thread (spec §4.1 forced reprobe, §5.2)

**Deliver — core (yserver-core):** a new `Backend` entry for RANDR's forced
reprobe that may complete synchronously or return a pending token (the
`begin_crtc_config` / `CrtcConfigApply` pattern), whose default keeps every
other backend on today's synchronous `reprobe_connectors`. On `Pending`, the
core **parks the `GetScreenResources` reply and holds the gate turn** (a
forced-reprobe flight), wakes on the backend's ready signal, then: publishes
the result (if any) in the turn, replies from `state.randr`, releases the turn.
A token resolved `Failed(io)` replies `BadAlloc` (today's mapping). A token
resolved `Expired` replies from the published state. A requester that
disconnects while parked leaves the backend work to finish or be discarded
without a reply; the turn is released.

**Deliver — backend:** on a server with an Owner device, the forced reprobe
starts a probe episode of cause `ForcedReprobe` with Legacy's reprobe probe
(decision 4) and returns `Pending`. Resolved: the result is applied as
`publish_connector_probes` applies it — connection state and mode lists into
the registry, **no KMS change** (decision 8), `AdministrativeReprobe` recorded
for a changed device — and published in the turn, then the token is ready. At
the deadline: the probe epoch is **invalidated** (its answer is discarded
unapplied when it comes), the token resolves `Expired`, the turn ends, and a
**background `TopologyRebuild` probe** is armed (the debounced rescan) so a real
change is published requester-less in its own episode (spec §5.2). A probe
failure resolves `Failed` (Legacy's `BadAlloc`). `L_reprobe` leaves the
synchronous term of 3b-ii's bound (update the bound's accounting/comment where
it is stated).

| Test | Scenario | Must fail under |
| --- | --- | --- |
| `c0_3cii_core_forced_reprobe_parks_the_reply` (yserver-core) | the backend returns `Pending`: no reply until the ready signal; a client `SetCrtcConfig` queued behind it waits; after ready, the publication precedes the reply, then the queued request runs | **H41** reply before the backend's ready signal |
| `c0_3cii_core_forced_reprobe_requester_gone` (yserver-core) | the requester disconnects while parked: the turn is released when the token resolves, nothing is written to the departed client | **H42** keep the gate held after the requester left |
| `c0_3cii_forced_reprobe_parks_and_expires_vulkan` | Owner, the prober blocked past the deadline: the reply carries the published state at the deadline; the queued mutation proceeds; a background rescan is armed | **H43** leave the reply parked past the deadline |
| `c0_3cii_forced_reprobe_timeout_discards_late_result_vulkan` | after the expiry, a client mutation is validated against the old state and applied; then the late answer arrives: it changes nothing; the background rescan's own episode publishes the real change requester-less | **H44** apply the late forced-reprobe answer |
| `c0_3cii_forced_reprobe_never_commits_vulkan` | the forced reprobe finds an active output's connector gone: registry and publication as Legacy's reprobe, no transition, no executor send, the output stays enabled until the udev edge's episode | **H45** run a lifecycle transaction for a forced reprobe |
| `c0_3cii_forced_reprobe_failure_is_badalloc_vulkan` | the probe fails (`EIO`): the client receives `BadAlloc` as on Legacy; nothing published | **H46** reply success on a failed probe |
| `c0_3cii_legacy_only_reprobe_unchanged` | Legacy-only server: `GetScreenResources` runs `reprobe_connectors` synchronously (recorded seam) | **H47** route a Legacy-only reprobe through the worker |
| `c0_3cii_background_rescan_waits_for_stuck_worker_vulkan` | *(rev 2, B-2)* the forced reprobe expires with its worker still stuck: the background rescan stays pending (no episode fails on the stuck device); the worker returns and is joined; one fresh episode publishes the real change requester-less | **H55** start the background rescan while the worker is stuck |
| `c0_3cii_release_resolves_parked_forced_reprobe_vulkan` | *(rev 2, M-2)* a forced reprobe is parked; `on_vt_release`: the token resolves `Expired`, the requester gets the published state, the turn is released, the late answer is discarded and no background rescan is armed while released | **H56** leave the forced reprobe parked across a release |

**Hardware (rev 2, M-3):** this task writes **`c0_hw_3cii_forced_reprobe_on_card1_drm`**
(written, never run by the implementer): opens card1 as Owner, drives
RANDR's forced reprobe through the new `Backend` entry with the production
prober: the reply resolves before the deadline, carries HDMI-2 with the same
mode list the synchronous `probe_connectors` returns, and no executor send
happens; it records the probe's duration. No cable, no VT; the coordinator
runs it × 3 after this task.

## Task 7 — the forced-reprobe differential, coverage and the final hardware run (spec §6.3, §6.5)

**Deliver:** the Legacy/Owner differential for the forced reprobe; the
`topology` writer coverage flips to proven, citing this plan's tests and naming
the deferred rows (claiming nothing for them); `docs/phase-c0-deferred-real-server-tests.md`
gains the 3c-ii rows: `DeviceRemoved` and `DeviceAddedOrReplaced` on real
hardware (a card unbound/removed and added), the real udev monitor delivering a
card node `remove`/`add`, and hotplug/forced reprobe through the assembled
server on Owner (stage 5).

| Test | Scenario | Must fail under |
| --- | --- | --- |
| `c0_3cii_forced_reprobe_differential_vulkan` | Legacy vs Owner, a mode list changes and a connector appears, then `GetScreenResources`: reply bytes, events to every installed client and `lastConfigTime` equal | **H48** publish the reprobe through the hotplug-episode publication path (different notifications) |
| `c0_3cii_topology_writer_coverage_proven` | the coverage evidence reports `topology` proven, citing the tests; the deferred rows are named, not claimed | **H49** leave `topology` unproven while claiming the evidence |

Coordinator (C), after every task on a real path *(rev 2, M-3)*:
`c0_hw_3cii_probe_worker_on_card1_drm` × 3 (after Task 1, no cable, no VT);
`c0_hw_3c_hotplug_on_card1_drm` × 3 with the user unplugging and replugging
HDMI-2 (after Tasks 2, 3, 4 and 7); `c0_hw_3c_vt_switch_on_card1_drm` × 3 from
a tty (after Tasks 5 and 7); `c0_hw_3cii_forced_reprobe_on_card1_drm` × 3
(after Task 6, no cable, no VT); `c0_hw_3b` × 3 for regression (after Tasks 2
and 7). The modeset and probe tests need no tty and are run by the
coordinator; only the VT test needs the user at a text VT. The final run
reports the measured probe durations against the 2 s deadline.

## Gate (every task)

`cargo +nightly fmt` first, then `cargo +nightly fmt --check` (never
`--check` on an unformatted tree: it exhausted memory once); `cargo clippy
--all-targets -- -D warnings` in default, `--features tcp-transport`,
`--features xdmcp`; each filter above with `--include-ignored --skip _drm`;
`--lib -- --skip c0_2ci --skip _drm`; `--lib c0_2ci -- --skip _drm`;
`-p yserver-core -- --skip _drm`; every integration file except
`render_acceptance` with `--skip _drm`. Report exact counts per line. Known
intermittent failures (plan 3b-i-2 rev 4, plus
`c0_3bi_position_only_updates_in_place`): a run failing only on those is
re-run once; a repeat, or any other failure, is a finding. Before finishing a
task, grep the task's diff for `cfg(test)` inside functions a `_drm` test
reaches, and report each one.

## For the user

- **Spec correction (decision 2):** spec §4.1 says the probe worker needs no
  master. The kernel only does the forced probe (EDID re-read, fresh modes) for
  the current master, so the worker uses a `dup` of our master fd instead of a
  fresh open. The spec's intent (off the core thread) is unchanged.
- **Fd tracking (decision 3):** the spec's "incarnation fd set" does not exist
  in production; the plan tracks workers in its own per-device ledger and
  leaves `IncarnationFdSet` wiring to 3d.
- **Forced reprobe (decision 8):** on Legacy a forced reprobe never modesets;
  the plan keeps that on Owner, so `AdministrativeReprobe` is recorded but
  starts no transaction. Change it if the spec's table meant a commit.
- **Real monitor test (Task 7):** it rides on the physical HDMI replug in the
  hardware test, because the udev library here refuses injected messages from
  an unprivileged process.
