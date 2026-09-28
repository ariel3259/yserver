# Stage 3c-ii — connector hotplug and device removal on the Owner

> **Implementer:** codex (model `gpt-6-luna`, reasoning effort `xhigh`; `max` from the first send-back), run with `< /dev/null` inside `systemd-run --user --scope -q -p MemoryMax=16G -p MemorySwapMax=0`. Hard rules, restated in every prompt: **no git write commands** (the coordinator verifies and commits); of the `#[ignore]` tests run only this plan's filters, each by its own command — `c0_3cii_`, `c0_3ci_`, `c0_3bi_`, `c0_3bii_`, `c0_3aii_`, `c0_3a_`, `c0_2b_add_`, `c0_conv_ciii_`, `c0_conv_cii_`, `c0_conv_cfb_`, `c0_conv_cp_`, `c0_adm`, `c0_merge_` — with `--include-ignored --skip _drm` only when the prompt records the user's GPU approval; **never** `_drm` tests, `render_acceptance`, an unfiltered `--ignored`, a VT switch, or anything that performs a modeset or takes DRM master: the hardware tests are **written, never run**, by the implementer; no deletes outside the worktree; remove temporary instrumentation before finishing; never edit `docs/status.md`. **You write the implementation and the tests**; this plan gives the interfaces, the invariants, the named tests with the scenario each must exercise, and the mutations each must catch. Execute tasks in order, one at a time; stop with the tree dirty after each task. **Do not ask for approval inside a run** — if the plan leaves a real design choice open, or something it states does not hold in the code, the kernel or C.0, stop and report it (F8); never silently substitute a test shape, never weaken an existing assertion.

**Review loop closed (user, 2026-09-28) after round 6.** New convergence
criterion (user): the loop runs while rounds find **design** defects
(architecture, cross-task or cross-crate contracts, ownership of a resource or
turn, task order); **edge cases are validated in implementation** — the
implementer writes the test, sees it fail, fixes it in its task. Rounds 1–2
found the design defects; rounds 3–6 found only edge cases, several opened by
the coordinator's own policy patches. Round 6 (3 blocking, 0 major, all edge
cases, `../findings/2026-09-28-stage-3c-ii-plan-review-round6.md`) is carried
as implementation notes **IN-1** (Task 1), **IN-2** (Task 4) and **IN-3**
(Task 5), each stated as an invariant with the test that must prove it.

**Revision 6 (2026-09-28, coordinator)** — codex round 5 (3 blocking, 1 major,
all confirmed, `../findings/2026-09-28-stage-3c-ii-plan-review-round5.md`): a
failed Owner acquire answer is closed and withdrawn on receipt, not at
resolution (B-1, spec §3.3); the hotplug commit disables with the same
`preserves_active_output` predicate that decides KMS work (B-2); a forced
reprobe while released probes on the worker, which the kernel demotes to the
cached read Legacy also gets, so no parity exception remains (B-3, replacing
rev 5's rule); a forced reprobe waits for an open probe episode, its deadline
counted from the request (M-1).

**Revision 5 (2026-09-28, coordinator)** — codex round 4 (1 blocking, 1 major,
confirmed, `../findings/2026-09-28-stage-3c-ii-plan-review-round4.md`; all
eleven earlier findings audited, round 3's B-1 PARTIAL): KMS work uses Legacy's
own `preserves_active_output` predicate, so a present connector with no mode
loses its route (B-1); a forced reprobe requested while the seat is released
answers at once from the published state (M-1).

**Revision 4 (2026-09-28, coordinator)** — codex round 3 (1 blocking, 1 major,
confirmed, `../findings/2026-09-28-stage-3c-ii-plan-review-round3.md`; all nine
findings of rounds 1–2 audited APPLIED): Task 2 judges KMS work against the
installed and remembered routes, not only the registry a forced reprobe may
already have updated, with a forced-reprobe-then-edge test in Task 5 (B-1);
Task 1's stuck-worker test asserts the §3.3 closure and the reaping, and the
no-second-worker mutation moves to Task 2's still-open hotplug scenario (M-1).

**Revision 3 (2026-09-28, coordinator)** — codex round 2 (2 blocking, 2 major,
all confirmed, `../findings/2026-09-28-stage-3c-ii-plan-review-round2.md`),
written as one fresh unit rather than patched: a result consumed at
`poll_deferred_input` wakes the core again for what it enqueued, because the
core drains publications only after a `CrtcConfigReady` and the loop tail has
no drain (B-1, decision 5); episodes are never delayed by a stuck worker — the
stuck device fails at once as the spec says — and a failure caused by a stuck
worker arms one retry that runs when the worker is joined (B-2, decision 6);
the first consumer of the worker is now the **acquire**, whose application 3c-i
already delivers, and the hotplug route switches in one task with its whole
application (M-1: rev 2's Task 1 consumer only logged, and rev 2's Task 5 is
folded into Task 1); the forced reprobe owns its request's turn and never
signals `EpisodeBegin` — "probe episode" and "topology episode" are now
distinct terms (M-2, decision 13). Six tasks. Round 1's corrections stand
(decisions 10–12, hardware after every real-path task).

**Revision 2 (2026-09-28)** — codex round 1 (2 blocking, 3 major,
`../findings/2026-09-28-stage-3c-ii-plan-review-round1.md`): the episode's turn
is granted by the core; a retry waits for a stuck worker; `ENODEV` on receipt;
a release invalidates every outstanding probe; hardware per real-path task.

**Revision 1 (2026-09-28).**

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
(`../findings/2026-09-27-stage-3c-i-accepted.md`) — the urgent withdrawal, the
episode turn (`EpisodeBegin`/`EpisodeEnd`), `AcquireEpisode`, the per-device
acquire probe — and on the branch tip `913508e1` (upstream merges through
`joske/master` `c208ade5`).

## Global constraints

- Production stays `Legacy`. A **Legacy-only** server keeps today's
  synchronous `run_display_rescan`, `reprobe_connectors`,
  `on_display_hotplug` and `on_vt_acquire_legacy` byte for byte (a test pins
  each). Everything below applies to a server with at least one Owner device.
- The client contract is parity with Legacy; the only differences are the
  spec's §5 exceptions (2 and 4 here; 1, 3, 5 are 3c-i's).
- **Probe deadline: 2 s** (`PROBE_EPISODE_DEADLINE`), for every probe episode
  (acquire, udev edge, forced reprobe). The hardware tests record the probe
  duration; the constant changes only if a measurement exceeds it (then F8, not
  a silent raise).
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
  results enter through a runtime seam chosen at construction (decision 4),
  never through a `cfg(test)` branch inside a function the hardware test also
  runs.
- **A test in task N uses only what tasks 1..N deliver.** A scenario that needs
  a later task's mechanism lives in that later task.

## Design decisions this plan fixes

1. Test names start with `c0_3cii_` (the two carried 3c-i tests keep their
   `c0_3ci_` names); Vulkan tests end in `_vulkan` and use the 3b/3c-i fixtures
   (`owner_live_fixture*`, the two-Owner position fixture, the mixed
   Legacy+Owner fixture of 3b-i-2 Task 4). Mutations are numbered **H1…**
   (numbers are stable across revisions; gaps are retired mutations).
2. **The worker's fd is a `dup` of the device's master fd, not a fresh open of
   the node.** *(Correction to spec §4.1, whose parenthetical says no master is
   needed; confirmed by codex round 1.)* The kernel performs the forced probe
   (`fill_modes`: EDID re-read, fresh mode list) only when the calling
   `drm_file` is the current master; otherwise it logs "demoting to read-only
   probe" and returns cached state
   (`~/Projects/linux/drivers/gpu/drm/drm_connector.c:3376-3386`,
   `drm_mode_getconnector`). A `dup` (`F_DUPFD_CLOEXEC`) shares the open file
   description, hence the `drm_file`, hence master. The acquire probe runs
   after `drmSetMaster` and hotplug probes never run while the seat is
   released (decision 12), so their worker's fd is master. The one probe that
   runs while released — a forced reprobe (decision 12, Task 5) — gets the
   kernel's read-only demotion, **exactly** as Legacy's synchronous probe does
   on its non-master fd; that is parity, not a defect.
3. **Fd tracking.** Spec §4.1 says the worker's fd is "registered in the device
   incarnation's fd set (C.0's tracked fd ownership)". `IncarnationFdSet`
   (`kms/executor/mod.rs`) has **no production instance** today (it is
   `#[allow(dead_code)]`, used only by its own unit tests; confirmed by codex
   round 1). This plan does not wire it; it adds a per-device
   **`ProbeWorkerLedger`** in the backend that owns each worker's `JoinHandle`
   and its fd lifetime: the fd moves into the worker thread and is closed when
   the worker's call returns; a worker that missed its deadline stays in the
   ledger as **stuck** until it returns, then it is joined. The ledger is
   retired with the device (removal keeps a stuck entry until it returns; 3d
   owns incarnation teardown). Wiring the ledger into `IncarnationFdSet` is
   3d's.
4. **The probe seam.** A `ConnectorProber` (trait object or enum, the
   implementer's choice) is chosen when the backend is built: production
   performs the real ioctls on the worker's fd; fixtures install a scripted
   prober whose per-device answers are `Ok(snapshot)`, `Err(errno)` —
   including `ENODEV` — or **block on a barrier the test controls** (a real
   worker thread really blocked, not a scripted late reply). The worker runs
   the same probe function the Legacy site it replaces runs:
   `probe_connector_snapshots` for the acquire and the udev edge,
   `probe_connectors` for the forced reprobe (Legacy's `reprobe_connectors`
   uses the lighter probe, and parity needs the same data).
5. **Result delivery and the core's drain.** A worker sends its tagged result
   (`DrmDeviceKey`, incarnation, probe epoch, the result) on a channel the
   backend owns and wakes the core through the existing `CoreSender`
   (`Message::CrtcConfigReady`). The backend consumes results at its
   state-carrying periodic entry (`poll_deferred_input`, where the debounced
   rescan fires today). *(Rev 3, B-1.)* The core drains requester-less
   publications, urgent withdrawals and topology-episode events **only** in
   its `CrtcConfigReady` handling and after the VT entries
   (`core_loop/run.rs`), and `poll_deferred_input` runs in the iteration tail
   (`run_iteration_tail`) with no drain after it — so the worker's own wake has
   already been serviced when its result is consumed. Therefore: **whenever
   consuming results enqueues anything the core drains** (an urgent
   withdrawal, an `EpisodeBegin`/`EpisodeEnd`, a publication, a ready forced
   reprobe token), the backend sends one more `CrtcConfigReady` after
   enqueueing — the producer contract `Backend::drain_requesterless_publications`
   already documents. Invariant: a single worker answer with no other stimulus
   reaches clients within the next loop iteration. The episode deadline is part
   of `next_wakeup`; no polling timer is added.
6. **Episodes are never delayed; a stuck worker arms one retry.** One probe
   episode at a time per server: an edge arriving while a probe episode or a
   topology episode is open is recorded and re-arms the debounced rescan when
   that episode ends. *(Rev 3, B-2.)* An edge (or the background rescan of
   decision 8) always **starts its episode at once**; a device with a stuck
   worker fails in it **at once** (spec §4.1: "a new probe of that device fails
   at once … the episode applies the failure rule"). Any episode that failed
   because a device had a stuck worker, or whose deadline left a worker stuck,
   **arms one retry**: when that worker returns and is joined, one fresh probe
   episode starts (if none is open and the seat is held). Retries do not
   accumulate: at most one is pending per server. *(Rev 6, M-1 of round 5.)*
   **A forced reprobe that arrives while another probe episode is open** (a
   hotplug or background-rescan probe still collecting answers) parks its
   reply and holds its request's turn, **waits for that episode to resolve**,
   then starts its own probe episode; its deadline (`PROBE_EPISODE_DEADLINE`)
   runs from the request, covering both waits. This cannot deadlock: a probe
   episode resolves without the gate turn, and the hotplug topology episode it
   may lead to queues its `EpisodeBegin` behind the forced-reprobe flight
   (decision 10).
7. **A lifecycle transition only when there is KMS work.** Classification
   (§4.2) is computed for every changed device. A device whose change needs a
   KMS commit — an installed output the answer does not preserve
   (`preserves_active_output`: connector gone, or present with no mode), or a
   remembered route to relight — projects `IdentityChangingHotplug` or `TopologyRebuild`
   into its arbiter and runs one transaction (§4.3). A device whose change is
   logical only (a connector appeared with no remembered route, an inactive
   connector left, a mode list changed on an inactive connector) projects no
   transition: its registry change is staged into the episode and the
   participant is terminal at once, as Legacy publishes the same change
   without a modeset. No empty atomic commit is ever sent.
8. **The forced reprobe changes no KMS state** (Legacy parity:
   `reprobe_connectors` → `publish_connector_probes` never modesets and never
   disables an enabled output; confirmed by codex round 1). On Owner its probe
   moves off the core thread and its result is published in its gate turn;
   its classification is `AdministrativeReprobe`, recorded, and **projects no
   lifecycle transition**. A physically vanished active output found by a
   forced reprobe is left to the udev edge's episode, as on Legacy. At its
   deadline it arms a background rescan (spec §4.1), an ordinary hotplug
   episode.
9. **Hotplug kinds never reach the DPMS-shaped description.** Today an
   arbiter transition of a hotplug kind would fall into
   `lifecycle_topology_description` (`render/admission.rs`, the `ACTIVE`-only
   DPMS/VT-release shape) through `admission_dispatch_lifecycle_topology`.
   Task 2 gives `IdentityChangingHotplug` and `TopologyRebuild` their own
   prepared description; no task before Task 2 projects either kind.
10. **A topology episode's turn is granted, not taken.** Today the core's
    `begin_topology_episode` sets the episode as soon as it drains
    `EpisodeBegin`, even while a client mutation is in flight, and
    `EpisodeEnd` publishes regardless (`core_loop/run.rs`). From Task 2, an
    `EpisodeBegin` is a request: the core **grants** it when no mutation is in
    flight (queued behind the in-flight one otherwise, ahead of later waiters)
    and tells the backend through a new `Backend` notification carrying the
    episode id; the backend **stages no logical change and dispatches no
    commit** for any participant before the grant. An `EpisodeEnd(id, None)`
    for an episode not yet granted withdraws the request. 3c-i's acquire
    episode keeps working unchanged: its `EpisodeBegin` is consumed before
    pending requests are drained and, with the seat released, no client
    mutation can be in flight, so it is granted at once (a test pins this). An
    urgent withdrawal bypasses both.
11. **`ENODEV` is consumed on receipt.** A probe result answering `ENODEV` is
    acted on when the backend consumes it, not when the episode resolves: the
    device leaves the episode at once and its withdrawal does not wait for the
    other devices' answers or the deadline; the other devices' outcome
    (complete, failed, late) cannot hide it. Until Task 4 the action is the
    acquire's failed-probe disposition (close and urgently withdraw, 3c-i);
    from Task 4 it is `DeviceRemoved`.
12. **A VT release invalidates every outstanding probe.** At the release's
    prompt obligations, any open probe episode — acquire, hotplug, forced
    reprobe — has its epoch invalidated (late answers are discarded; a stuck
    worker stays in the ledger and a pending retry is dropped); a hotplug
    topology episode not yet granted ends with `EpisodeEnd(id, None)` and its
    edge is recorded for the acquire (spec §3.2); a granted hotplug episode
    whose transactions are dispatched follows 3c-i's release rules for
    dispatched work; a parked forced reprobe resolves `Expired` (the reply
    carries the published state, the turn is released) and **no background
    rescan is armed** while released — the acquire probe covers it. No
    acquire or hotplug worker is spawned while the seat is released; *(rev 6,
    B-3 of round 5)* a forced reprobe **requested** while released does run
    its probe episode on the worker, which the kernel demotes to a read-only
    probe of cached connector state — the same data Legacy's synchronous probe
    returns while released (decision 2) — so its reply matches Legacy's.
13. *(Rev 3, M-2.)* **Two kinds of episode, two owners.** A **probe episode**
    is the worker-level collection of per-device answers (epoch, deadline,
    ledger) — causes `Acquire`, `Hotplug`, `ForcedReprobe`. A **topology
    episode** is the gate-level turn (`EpisodeBegin`/`EpisodeEnd`, decision 10)
    — causes `Acquire` and `Hotplug` only; 3c-i's `AcquireEpisode` becomes this
    one type with a cause. **The forced reprobe never opens a topology episode
    and never signals `EpisodeBegin`:** it runs its probe episode inside the
    turn its own parked `GetScreenResources` request already holds (Task 5),
    publishes in that turn, and releases it with the reply.

## Review Focus

1. HDMI unplugged while a client `SetCrtcConfig` on another device is
   dispatched and unanswered (Task 2
   `c0_3cii_hotplug_on_b_waits_for_modeset_on_a_vulkan`).
2. A monitor whose EDID read hangs in the kernel: the worker is stuck, the
   next edge fails that device at once, the server keeps serving clients, and
   the change is still found once the worker returns (Task 2
   `c0_3cii_edge_fails_stuck_device_fast_then_retries_vulkan`).
3. Unplug and replug inside the 150 ms debounce, or a replug while the unplug's
   episode is still open (Task 2 `c0_3cii_edge_during_open_episode_rearms_vulkan`).
4. A GPU unbound while the VT is away, then the user switches back (Task 4
   `c0_3ci_acquire_skips_a_removed_device_vulkan`,
   `c0_3cii_removal_while_released_is_not_deferred_vulkan`).
5. A desktop that polls `GetScreenResources` every second while a monitor is
   plugged in (Task 5 `c0_3cii_forced_reprobe_parks_and_expires_vulkan`,
   `c0_3cii_forced_reprobe_never_commits_vulkan`).

---

## Task 1 — the probe worker, the probe episode and the acquire probe off the core thread (spec §4.1 worker rules, §3.3)

*(Rev 3, M-1: the worker's first consumer is the acquire, whose whole
application 3c-i already delivers, so no route is ever half-switched; rev 2's
Task 5 is folded in here.)*

**Deliver — the worker:** per probe, a fresh worker thread per device with a
`dup` of the device's master fd (decision 2), recorded in the device's
`ProbeWorkerLedger` (decision 3), running the installed `ConnectorProber`
(decision 4), returning a result tagged with `DrmDeviceKey`, incarnation and
probe epoch, delivered as decision 5 says. While a device has a **stuck**
worker, a new probe of that device fails **at once** with no second thread.

**Deliver — the probe episode:** one episode covers every open device that is
not closed or removed; it has a probe epoch and an absolute deadline (start +
`PROBE_EPISODE_DEADLINE`) armed in `next_wakeup`. A result is **stale** — and
discarded without effect — when its epoch is not the open episode's or its
incarnation is not the device's current one. The episode resolves when every
remaining device has answered or at the deadline; a device with no answer at
the deadline counts as failed, and the epoch is invalidated so its late
answer is discarded. An `ENODEV` answer takes the device out of the episode
on receipt (decision 11). The retry of decision 6 and the release rule of
decision 12 apply to every episode from this task on.

**Deliver — the acquire on the worker:** on a server with an Owner device,
`on_vt_acquire` keeps its order up to master (the `AcquireEpisode` begins first
and the core reserves its turn before draining requests; `VT_ACKACQ`; bounded
`drmSetMaster` over the devices still present), then starts a probe episode of
cause `Acquire` and **returns**. The rest of 3c-i's acquire — scoped Legacy
resume, the per-participant dispositions of spec §3.3's table, `VtState::Active`,
input resume, xkb resync, the reinstalls — runs as the **continuation** when
the probe episode resolves. The acquire keeps §3.3's per-participant rule, not
the all-or-nothing boundary: a Legacy failure or no answer keeps today's exit;
a healthy Owner device with an answer reinstalls; an Owner device that failed,
did not answer by the deadline, or answered `ENODEV` (decision 11) is closed
and urgently withdrawn. *(Rev 6, B-1 of round 5.)* Spec §3.3 gives each
participant its disposition **once its probe answers**: a failed Owner
answer (`EIO` or `ENODEV`) is closed and urgently withdrawn **on receipt**,
and leaves the episode; only the healthy Owner reinstalls and the ordered
continuation (scoped Legacy resume, input resume) wait for the episode to
resolve. While the probe is
outstanding `VtState` stays `Resuming`, input stays paused, clients are served
from the published state. A release arriving before the continuation
supersedes it (decision 12): the episode ends with `EpisodeEnd(id, None)`,
nothing is reinstalled, the hand-off follows 3c-i. The synchronous
`probe_connector_snapshots_per_device` is no longer called on this path;
3c-i's tests that scripted it move to the decision 4 seam with their assertions
intact. Hotplug edges recorded while released are covered by this probe and
the reinstall.

| Test | Scenario | Must fail under |
| --- | --- | --- |
| `c0_3cii_acquire_probe_is_off_the_core_thread_vulkan` | the prober blocked: `on_vt_acquire` returns; input is still paused and `VtState` is `Resuming`; a client query is answered from the published state; the barrier opens: the continuation runs and input resumes | **H35** probe synchronously in `on_vt_acquire` |
| `c0_3cii_worker_answer_alone_reaches_clients_vulkan` | *(rev 3, B-1)* acquire with a topology change while away; the barrier opens and **nothing else happens** (no client request, no other fd): the continuation runs, the reinstall is dispatched and, once `Applied`, the episode publication reaches an installed client — every step through the driver's modelled wakes only; the backend sends `CrtcConfigReady` after each enqueue | **H57** omit the self-wake after consuming a result |
| `c0_3cii_probe_worker_shares_the_master_file` | the fd handed to the prober shares the open file description of the device's master fd (proved by `kcmp(KCMP_FILE)`, or a file-status flag set on one fd visible on the other) | **H2** open the device node afresh for the worker |
| `c0_3cii_stale_probe_discarded_vulkan` | an acquire probe episode is superseded by a release and a new acquire; the first episode's answer arrives during the second, and a result tagged with an older incarnation arrives: neither changes the second episode's results or its dispositions | **H3** accept a result without checking epoch and incarnation |
| `c0_3ci_acquire_probe_no_reply_withdraws_owner_device_vulkan` | two Owner devices, B never answers: at the probe deadline (through `next_wakeup`) A reinstalls and composes, B is closed and urgently withdrawn, the episode publishes A only; the server continues; the late answer changes nothing | **H4** leave the deadline unarmed / apply the late answer; **H36** end the server (or wait forever) when an Owner device does not answer |
| `c0_3cii_stuck_acquire_worker_closes_and_is_reaped_vulkan` | *(rev 4, M-1 of round 3)* B's worker is stuck past the acquire's deadline: B is closed and urgently withdrawn (spec §3.3) while its worker stays in the ledger as stuck; release and acquire again: B, being closed, is not probed (no new worker), A probes and reinstalls; the barrier opens: B's worker returns, is joined, its fd closed, the ledger empty, and its answer changes nothing | **H61** probe a closed device at the next acquire; **H62** never join a stuck worker |
| `c0_3cii_acquire_enodev_on_receipt_vulkan` | two Owner devices; B answers `ENODEV` while A's worker is blocked: B's urgent withdrawal reaches the listener **before** A answers; A then answers and reinstalls; second case, B answers `EIO` while A is blocked: B is likewise closed and urgently withdrawn before A answers (a failed probe, not `ENODEV`: from Task 4 it raises no `DeviceRemoved`) | **H6** classify every probe error as `ENODEV`; **H6b** act on `ENODEV` or `EIO` only at episode resolution |
| `c0_3cii_release_during_acquire_probe_vulkan` | acquire, then release before the probe answers: no reinstall reaches the executor, the episode aborts without publishing, hand-off as 3c-i; the late answer changes nothing; no worker is spawned while released | **H39** run the continuation after a superseding release |
| `c0_3cii_hotplug_while_released_waits_for_acquire_vulkan` | while released, A's connector vanishes (edge recorded, no worker spawned); at acquire the probe sees it, the reinstall installs the remaining outputs and one publication shows the change | **H38** probe while the seat is released |
| `c0_3cii_mixed_acquire_legacy_failure_on_receipt_vulkan` | **IN-1** (round 6 B-2): mixed Legacy + Owner; the Legacy device's probe answers `EIO` while the Owner worker is blocked: today's Legacy exit is requested on receipt, not after the Owner answer or the deadline (spec §3.3: every participant's disposition once its probe answers) | **H68** defer the Legacy failure to episode resolution |
| `c0_3cii_legacy_only_acquire_unchanged` | Legacy-only server: `on_vt_acquire` runs today's synchronous route (recorded seam), no worker is spawned | **H40** route a Legacy-only acquire through the worker |

**Hardware:** this task writes **`c0_hw_3cii_probe_worker_on_card1_drm`**
(written, never run by the implementer): opens card1 as Owner with the
production prober, runs one probe episode through the real worker, and asserts
its snapshot equals the synchronous `probe_connector_snapshots` on the same fd,
that the worker was joined, the ledger is empty and the dup'd fd is closed; it
records the probe's duration. The coordinator runs it × 3 (no cable, no VT),
and the user runs `c0_hw_3c_vt_switch_on_card1_drm` × 3 from tty4
(`sh ~/vt.sh`) — the acquire's probe now runs on the worker.

## Task 2 — the Owner hotplug route: classification, transaction, turn and episode (spec §4.1–§4.4)

Co-delivered: the Owner hotplug route switches on only with its whole
application, so no tree exposes a half route.

**Deliver — the baseline and classification** *(rev 4, B-1 of round 3)*: a
complete answer is judged against **two baselines, never only the RANDR
registry** — Task 5's forced reprobe updates the registry without touching any
installed output (decision 8), so a registry that already says "disconnected"
must not hide an unplug. **KMS work** is decided against the installed and
remembered routes, as Legacy's rescan does (`apply_connector_snapshot` drops
every installed output absent from the snapshot even when a forced probe
already marked its connector disconnected, `render/platform.rs`): a device has
KMS work when an installed output is **not preserved** by the answer — no
answer entry satisfies Legacy's own predicate
`ConnectorSnapshot::preserves_active_output` (same connector, **at least one
mode**; a connector still present but advertising no mode loses its route, as
on Legacy) *(rev 5, B-1 of round 4)* — or a remembered route's connector is
present with a compatible mode, **whatever the registry says**. **Logical change** is decided against the registry
(connection bits, modes, EDID). A device with either starts the topology
episode. Classification of a changed device: connector set or EDID changed
(against the installed routes and the registry) → `IdentityChangingHotplug`;
only mode lists or other discovered objects changed → `TopologyRebuild`;
neither baseline differs → none. `desired.rs`'s `TopologyChangeClass` is the
vocabulary. (Task 5 adds the forced reprobe's `AdministrativeReprobe`.)

**Deliver — the route:** on a server with an Owner device, the debounced udev
rescan starts a probe episode of cause `Hotplug` instead of
`run_display_rescan`'s synchronous probe. Outcome: `Complete` when every
remaining device answered; `Failed` if any failed or missed the deadline
(Legacy's combined boundary — nothing is staged, no topology episode, nothing
published); an `ENODEV` device left the episode on receipt (decision 11; until
Task 4, it is handled like the acquire's failed probe: closed and urgently
withdrawn). Decision 6 applies: a stuck device fails at once and arms the
retry.

**Deliver — the grant (yserver-core + backend):** decision 10. A `Complete`
outcome with at least one changed participant signals `EpisodeBegin`; the core
grants it when no mutation is in flight and notifies the backend through a new
`Backend` entry (default no-op for other backends); **nothing is staged or
dispatched before the grant**, so a client modeset in flight on **any** device
is terminal and published before the episode stages its change.

**Deliver — the staged logical change:** after the grant, the backend computes
per device the same logical result Legacy's rescan computes (apply the
snapshot, reconcile the registry, relight requests for returned remembered
routes, layout policy with reserved slots and extent — the existing shared
logical code) **as a staged change** the backend model adopts only at that
participant's terminal result. Legacy participants of a mixed server run
today's rescan steps **scoped** to them (`_for_devices` helpers) synchronously
inside the turn, and **stage** their RANDR difference into the episode instead
of publishing it (3c-i's scoped resume pattern).

**Deliver — the transaction, per Owner device with KMS work** (decision 7):
project `IdentityChangingHotplug` or `TopologyRebuild` through
`project_device_intent`; at `PhysicalAdvanceAllowed` the admission dispatch
builds a **hotplug description** (decision 9): *(rev 6, B-2 of round 5)*
every installed output the answer does not preserve — the **same**
`preserves_active_output` predicate that decided KMS work, so connector gone
**or** present with no mode — is disabled (its CRTC off, connector detached), every relit
remembered route is enabled with its mode, CRTC and a composed primary from a
freshly prepared pool — 3b's execution (preparation, infallible promotion, a
retired bundle for each displaced pool waiting for its KMS proof) — and **kept
outputs contribute no object** to the commit. The DPMS projection is refreshed
first (3a): under protocol DPMS off, a relit output is installed with
`ACTIVE=0`. A root-storage change gives every kept output full damage and an
ordinary composed repaint, never a lifecycle object.

**Deliver — the topology episode** (decision 13): cause `Hotplug`. A
participant becomes terminal on: its transaction's `Applied` (the staged change
is adopted); **rejected with known completion** (its previous installed
topology is kept, except outputs whose connector is physically gone, which are
withdrawn logically); unknown (`CompletionUnknown`/stalled: the existing
`ExecutorStalled` closure and urgent withdrawal); logical-only (at once); a
Legacy participant when its scoped steps return. Then one
`EpisodeEnd(id, publication)` built from the backend model — `None` when
nothing published changed — delivered as decision 5 says.

**Deliver — the hardware test:** **`c0_hw_3c_hotplug_on_card1_drm`** (written,
never run by the implementer): opens card1 as Owner (it skips with a logged
reason if another process holds master), composes on HDMI-2, then × 4 prompts
the user on stderr to unplug HDMI-2 and waits (generous timeout, e.g. 60 s)
for the real udev edge → probe episode → transaction `Applied` → publication
without HDMI-2 and its pool retired; then prompts to replug and waits for the
relight `Applied`, a composed frame on HDMI-2 and a publication with it. It
records each probe's duration. Each cycle ends with the end state check. The
coordinator runs it × 3 after this task, with the user handling the cable, and
`c0_hw_3b` × 3 for regression.

| Test | Scenario | Must fail under |
| --- | --- | --- |
| `c0_3cii_classification` | unit: connector added, connector removed, same name with a new EDID → `IdentityChangingHotplug`; only a mode list changed → `TopologyRebuild`; identical → none | **H7** classify an EDID change as `TopologyRebuild` |
| `c0_3cii_probe_runs_off_the_core_thread_vulkan` | the prober blocked; the debounced rescan fires through the driver: the core entry returns while the worker is blocked, and a `GetScreenResourcesCurrent` from an installed client is answered from the published state before the barrier opens | **H1** run the hotplug probe inline in `poll_deferred_input` |
| `c0_3cii_unplug_retires_and_publishes_vulkan` | two lit outputs, one connector vanishes from the scripted probe: one commit disables its CRTC, its pool is retired into a bundle discharged by the KMS proof; one publication without it, reaching the client with no other stimulus after the worker's answer (decision 5); end state clean | **H9** destroy the pool without waiting for its proof |
| `c0_3cii_replug_relights_remembered_route_vulkan` | after an unplug, the connector returns with a compatible mode: one commit relights it with a fresh pool; a composed frame reaches it; one publication with it back at its remembered position | **H10** relight through the Legacy `enable_connector` path |
| `c0_3cii_kept_output_is_not_a_lifecycle_object_vulkan` | an unplug beside a kept lit output whose layout position changes: the commit names no object of the kept CRTC; the kept output gets full damage and an ordinary composed repaint | **H11** include the kept CRTC in the hotplug commit |
| `c0_3cii_hotplug_uses_its_own_description_vulkan` | a hotplug transition is dispatched: the committed description disables/enables connectors and planes, not the `ACTIVE`-only DPMS shape | **H12** build hotplug commits with `lifecycle_topology_description` |
| `c0_3cii_relight_under_dpms_off_vulkan` | protocol DPMS off, the remembered connector returns: installed with `ACTIVE=0`, no frame admitted | **H13** relight lit regardless of DPMS |
| `c0_3cii_zero_mode_connector_loses_its_route_vulkan` | *(rev 5, B-1 of round 4)* a lit output's connector is still present in the answer but advertises no mode: one commit detaches it and retires its pool, the connector stays connected in RANDR, one publication, as Legacy's rescan | **H64** decide KMS work on connector presence alone |
| `c0_3cii_logical_only_change_needs_no_commit_vulkan` | a connector appears with no remembered route: no transition, no executor send; one publication with the new connected output | **H14** send a commit (or an empty one) for a logical-only change |
| `c0_3cii_episode_publishes_once_after_every_commit_vulkan` | two Owner devices both change; A `Applied`, B held in flight: no RANDR bytes to clients and a client `SetCrtcConfig` stays queued; B terminal: exactly one publication, then the queued request proceeds | **H15** publish when the first participant applies |
| `c0_3cii_episode_partial_commit_failure_vulkan` | two Owner devices: A's commit rejected with known completion (a connector gone on A), B's commit unknown: A shows its previous topology minus the gone output, B is withdrawn (`ExecutorStalled`, one urgent withdrawal), the episode publishes once, per the outcome table | **H16** publish A's staged change after its rejection |
| `c0_3cii_episode_mixed_legacy_owner_vulkan` | Legacy + Owner, both change: the Legacy steps touch only the Legacy device and stage their difference; the Owner transaction runs; one publication carries both | **H17** let the scoped Legacy steps publish directly |
| `c0_3cii_one_failed_probe_applies_nothing_vulkan` | two devices, one probe fails (and, second case, one is late past the deadline): no staged change, no `EpisodeBegin`, no transition, no publication; the next edge starts a fresh episode that applies both | **H18** apply the devices that answered |
| `c0_3cii_edge_fails_stuck_device_fast_then_retries_vulkan` | *(rev 3, B-2)* B's worker is stuck from an earlier episode; an edge arrives: its episode **starts at once** and B fails in it at once — the ledger still holds exactly one thread for B — so nothing is applied; then the barrier opens: B's worker is joined and exactly **one** retry episode runs and applies the change | **H5** spawn a second worker for a device with a stuck one; **H51** hold the edge until the worker returns; **H58** drop the retry after the fast failure |
| `c0_3cii_hotplug_waits_for_dispatched_client_modeset_vulkan` | a client modeset dispatched on A and unanswered; A's connector vanishes: A's change is not staged and A's transaction not dispatched until the modeset is terminal; the modeset's reply and publication come first | **H19** stage the hotplug change against the pre-modeset model |
| `c0_3cii_hotplug_on_b_waits_for_modeset_on_a_vulkan` | a client modeset dispatched on A and unanswered; B's connector vanishes: B's change is not staged and no B commit is sent until A's modeset is terminal and published; then one episode publication for B | **H53** stage or dispatch before the core's grant |
| `c0_3cii_core_episode_begin_waits_for_in_flight_mutation` (yserver-core) | a mutation is in flight when `EpisodeBegin` arrives: no grant until the mutation finishes; a later client mutation queues behind the episode; `EpisodeEnd(None)` before the grant withdraws the request and the queue proceeds | **H54** grant `EpisodeBegin` while a mutation is in flight |
| `c0_3cii_acquire_episode_granted_at_once` (yserver-core + backend) | 3c-i's acquire: `EpisodeBegin` consumed before pending requests are drained is granted at once; 3c-i's acquire tests unchanged | **H59** queue the acquire's `EpisodeBegin` behind pending requests |
| `c0_3cii_edge_during_open_episode_rearms_vulkan` | a second edge arrives while the episode is open: no second episode starts; after `EpisodeEnd` the debounced rescan re-arms and its episode sees the newest state | **H20** start a concurrent probe episode |
| `c0_3cii_release_invalidates_outstanding_hotplug_probe_vulkan` | a hotplug probe is outstanding (and, second case, its topology episode awaits the grant); `on_vt_release`: the epoch is invalidated, an ungranted episode ends with `EpisodeEnd(None)`, the late answer is discarded, the edge is recorded for the acquire, no worker is spawned while released | **H52** keep the probe episode alive across a release |
| `c0_3cii_legacy_only_rescan_unchanged` | Legacy-only server: the debounced edge runs `run_display_rescan` synchronously with today's call sequence (recorded seam); no worker is spawned | **H8** route a Legacy-only server through the probe worker |

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

**Hardware:** the coordinator runs `c0_hw_3c_hotplug_on_card1_drm` × 3 after
this task if it changed production code (the user handles the cable).

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
`remove` or a probe answering `ENODEV` (decision 11; this task replaces the
interim failed-probe treatment of Tasks 1 and 2): projected through the
coordinator (`DesiredIntent::DevicePresence { present: false, .. }`); **no KMS
call** is attempted on that device; alias creation stops; executor/helper
termination is requested; `Invalidated(DeviceRemoved)` is recorded; its
outputs are withdrawn by **one urgent withdrawal at once** — never delayed by
an unreaped helper (which stays `ExecutorStalled` holding its handles and
quarantine until reap), by an open probe or topology episode (the device leaves
it; the topology episode publishes only its remaining participants), by a
mutation holding the gate, or by a released seat; every in-flight commit on it
terminates `CompletionUnknown` into quarantine and a client modeset token on it
resolves `Failed` through the ordinary ready-token path (3c-i); its probe
ledger is retired (a stuck worker stays until it returns); the arbiter reaches
`Removed`; the server continues. The urgent withdrawal is delivered as
decision 5 says. **The renderer's device** (the KMS device the selected Vulkan
renderer runs on, or the renderer's render node): the server exits, as on
device loss today. A Legacy device's removal is unchanged. A removed device is
neither asked for master nor probed at the next acquire.

**Deliver — `DeviceAddedOrReplaced`:** one log line; the card is not opened. A
removed device whose node reappears stays `Removed`.

| Test | Scenario | Must fail under |
| --- | --- | --- |
| `c0_3cii_udev_events_classified` | typed records as the monitor produces them: remove of the open card → `DeviceRemoved`; add of an unopened card → `DeviceAddedOrReplaced`; `change` with `HOTPLUG=1` and a connector sub-device event of the open card → edge; events for other subsystems' or unopened devices' `change` → nothing | **H23** raise `DeviceRemoved` for a `remove` whose `dev_t` is not open |
| `c0_3cii_device_removed_withdraws_and_continues_vulkan` | two Owner devices, B removed (classified event through the hotplug entry): no executor send on B afterwards, termination requested, one urgent withdrawal naming B's outputs and CRTCs reaching the client with no other stimulus, B `Removed`; A keeps composing and no exit is requested; end state clean except B's quarantine, named | **H24** issue a KMS call (e.g. an `ACTIVE=0` commit) on the removed device; **H24b** `request_exit` for a non-renderer removal |
| `c0_3cii_removal_via_enodev_vulkan` | a hotplug probe episode where B answers `ENODEV` while A's worker is still blocked: B is `Removed` and its withdrawal reaches the listener **before** A answers; then A answers and the episode applies A's change without B; second case, A fails: B is still removed and A's failure applies nothing else | **H25** treat `ENODEV` as an episode failure; **H25b** remove B only when the episode resolves |
| `c0_3cii_removal_while_released_is_not_deferred_vulkan` | the seat is released (3c-i release completed); B removed: withdrawn and published at once, not `Deferred(SeatReleased)` | **H26** defer the removal while released |
| `c0_3ci_acquire_skips_a_removed_device_vulkan` | B removed while released: at acquire B gets no `drmSetMaster` and no probe, does not trigger the exit, stays withdrawn; A reinstalls | **H37** ask a removed device for master |
| `c0_3cii_removal_during_unrelated_modeset_publishes_at_once_vulkan` | a client modeset on A holds the gate (dispatched, unanswered); B removed: the withdrawal's notifications reach the listener before the modeset's reply | **H27** queue the withdrawal behind the held gate |
| `c0_3cii_urgent_withdrawal_hides_unpublished_modeset_vulkan` | A's modeset is promoted in the backend but its publication has not run; B removed; a client reads `GetScreenResourcesCurrent` in between: it sees B gone and A's old configuration; after A's publication, both changes | **H28** build the withdrawal from the backend model |
| `c0_3cii_later_publication_does_not_restore_withdrawn_outputs_vulkan` | after B's withdrawal, A's queued publication and a later hotplug episode's publication run: B's outputs stay absent and no notification re-adds them | **H29** skip the withdrawn-id filter for backend-built publications |
| `c0_3cii_unknown_release_withdrawal_during_unrelated_modeset_vulkan` | through `on_vt_release`: A's release commit never answers while a client modeset on B holds the gate; at the deadline A's withdrawal is published without waiting for B's turn | **H30** publish an unknown-release withdrawal through the ordinary FIFO |
| `c0_3cii_removal_interrupts_episode_vulkan` | an open hotplug topology episode over A and B; B removed mid-episode: B leaves the episode, A's transaction completes, the episode publishes only A's change | **H31** keep waiting for the removed participant |
| `c0_3cii_renderer_removal_exits_vulkan` | the removed device is the renderer's: `request_exit` | **H32** keep running without the renderer |
| `c0_3cii_separate_renderer_removal_exits_vulkan` | **IN-2** (round 6 B-1): the selected renderer is a device whose primary node is not an open KMS card (a separate render endpoint); a udev `remove` of its card node or of its render node requests exit — the classifier correlates removal records with the selected renderer's primary- and render-node identities as well as the open KMS cards | **H69** classify removals against open KMS cards only |
| `c0_3cii_device_added_is_ignored` | an unopened card is added, and a removed card reappears: one log line each, no device opened, the removed one stays `Removed` | **H33** open the new card |
| `c0_3cii_probe_error_is_not_removal_vulkan` | a probe answering `EIO`, and a udev `change` for the open card: neither raises `DeviceRemoved` | **H34** raise `DeviceRemoved` from a probe failure other than `ENODEV` |

**Hardware:** `c0_hw_3c_hotplug_on_card1_drm` now also asserts the real
monitor's typed records — each physical unplug and replug of HDMI-2 yields a
`change` record with card1's `dev_t` and `HOTPLUG=1`, classified as a
connector edge, and no `DeviceRemoved` — as
**`c0_3cii_real_monitor_delivers_typed_events`** (**H50** derive the edge from
the action string alone, dropping `dev_t`). A fixture cannot prove the
kernel's delivery, and this box's systemd-261 libudev drops unicast from an
untrusted sender, so no unprivileged injection exists. The coordinator runs it
× 3 after this task, with the user handling the cable.

## Task 5 — the forced reprobe off the core thread (spec §4.1 forced reprobe, §5.2)

**Deliver — core (yserver-core):** a new `Backend` entry for RANDR's forced
reprobe that may complete synchronously or return a pending token (the
`begin_crtc_config` / `CrtcConfigApply` pattern), whose default keeps every
other backend on today's synchronous `reprobe_connectors`. On `Pending`, the
core **parks the `GetScreenResources` reply and holds the gate turn** (a
forced-reprobe flight — the only owner of that turn, decision 13), wakes on
the backend's ready signal (`CrtcConfigReady`, decision 5), then: publishes the
result (if any) in the turn, replies from `state.randr`, releases the turn. A
token resolved `Failed(io)` replies `BadAlloc` (today's mapping). A token
resolved `Expired` replies from the published state. A requester that
disconnects while parked leaves the backend work to finish or be discarded
without a reply; the turn is released when the token resolves.

**Deliver — backend:** on a server with an Owner device, the forced reprobe
starts a probe episode of cause `ForcedReprobe` with Legacy's reprobe probe
(decision 4) and returns `Pending`; it signals **no** `EpisodeBegin`. Resolved:
the result is applied as `publish_connector_probes` applies it — connection
state and mode lists into the registry, **no KMS change** (decision 8),
`AdministrativeReprobe` recorded for a changed device — its publication is
handed to the core with the ready token, and the token is ready. At the
deadline: the probe epoch is **invalidated** (its answer is discarded unapplied
when it comes), the token resolves `Expired`, the turn ends, and a
**background rescan** is armed (an ordinary `Hotplug` probe episode, decisions
6 and 8) so a real change is published requester-less in its own topology
episode (spec §5.2). A probe failure resolves `Failed` (Legacy's `BadAlloc`).
A release while parked follows decision 12. *(Rev 6, B-3 of round 5, replacing rev 5.)*
**A forced reprobe requested while the seat is released** follows the same
path — probe episode on the worker, parked reply, publication in its turn —
and the kernel demotes its probe to a read-only read of cached connector
state, which is exactly what Legacy's synchronous probe returns while released
(decision 2); the reply and publication therefore match Legacy's, and no spec
exception is needed. No background rescan is armed while released; hotplug
edges stay recorded for the acquire. `L_reprobe` leaves the synchronous
term of 3b-ii's bound (update the bound's accounting/comment where it is
stated).

| Test | Scenario | Must fail under |
| --- | --- | --- |
| `c0_3cii_core_forced_reprobe_parks_the_reply` (yserver-core) | the backend returns `Pending`: no reply until the ready signal; a client `SetCrtcConfig` queued behind it waits; after ready, the publication precedes the reply, then the queued request runs | **H41** reply before the backend's ready signal |
| `c0_3cii_core_forced_reprobe_requester_gone` (yserver-core) | the requester disconnects while parked: the turn is released when the token resolves, nothing is written to the departed client | **H42** keep the gate held after the requester left |
| `c0_3cii_forced_reprobe_owns_one_turn_vulkan` | *(rev 3, M-2)* Owner forced reprobe with a change: no `EpisodeBegin` is signalled; the publication and the reply happen in the request's own turn and the turn is released once; a client `SetCrtcConfig` queued behind runs after the reply | **H60** open a topology episode for the forced reprobe |
| `c0_3cii_forced_reprobe_parks_and_expires_vulkan` | Owner, the prober blocked past the deadline: the reply carries the published state at the deadline; the queued mutation proceeds; a background rescan is armed | **H43** leave the reply parked past the deadline |
| `c0_3cii_forced_reprobe_timeout_discards_late_result_vulkan` | after the expiry, a client mutation is validated against the old state and applied; then the late answer arrives: it changes nothing; once the stuck worker is joined, the retry's topology episode publishes the real change requester-less (decision 6) | **H44** apply the late forced-reprobe answer; **H55** lose the background rescan to the stuck worker |
| `c0_3cii_forced_reprobe_never_commits_vulkan` | the forced reprobe finds an active output's connector gone: registry and publication as Legacy's reprobe, no transition, no executor send, the output stays enabled until the udev edge's episode | **H45** run a lifecycle transaction for a forced reprobe |
| `c0_3cii_forced_reprobe_then_edge_still_detaches_and_relights_vulkan` | *(rev 4, B-1 of round 3)* a lit output's connector vanishes; a `GetScreenResources` forced reprobe records it disconnected in the registry first; then the udev edge's episode runs: it still detaches the output in one commit, retires its pool and publishes; second case, the replug is first seen by a forced reprobe, then the edge's episode relights the remembered route; third case, the connector stays present with zero modes, seen first by a forced reprobe, then the edge's episode detaches it | **H63** judge the edge's KMS work against the registry alone |
| `c0_3cii_forced_reprobe_failure_is_badalloc_vulkan` | the probe fails (`EIO`): the client receives `BadAlloc` as on Legacy; nothing published | **H46** reply success on a failed probe |
| `c0_3cii_release_resolves_parked_forced_reprobe_vulkan` | a forced reprobe is parked; `on_vt_release`: the token resolves `Expired`, the requester gets the published state, the turn is released, the late answer is discarded and no background rescan is armed while released | **H56** leave the forced reprobe parked across a release |
| `c0_3cii_forced_reprobe_while_released_matches_legacy_vulkan` | *(rev 6, B-3 of round 5)* the seat is released and the scripted prober (standing in for the kernel's cached state) reports a connector change; a client sends `GetScreenResources` on Legacy and on Owner: the Owner request runs its probe on the worker (no acquire/hotplug worker is spawned), and reply bytes, events and `lastConfigTime` equal Legacy's; no background rescan is armed while released | **H65** answer a released-seat forced reprobe from the published state without probing |
| `c0_3cii_forced_reprobe_waits_for_open_hotplug_probe_vulkan` | *(rev 6, M-1 of round 5)* a hotplug probe episode is outstanding (its worker blocked) when `GetScreenResources` arrives: the request parks and holds its turn, starts no second episode; the hotplug episode resolves and its topology `EpisodeBegin` queues behind the forced-reprobe flight; the forced reprobe then probes, publishes and replies, and only then is the hotplug episode granted; second case, the hotplug probe outlives the request's deadline: the reply carries the published state at the deadline | **H66** start a concurrent probe episode for the forced reprobe; **H67** measure the forced reprobe's deadline from the end of the wait |
| `c0_3cii_forced_reprobe_invalidates_older_hotplug_answer_vulkan` | **IN-3** (round 6 B-3): a hotplug probe episode resolves with an unplug while a forced reprobe waits; the connector returns before the forced probe, which publishes it connected; the hotplug topology episode queued behind the forced turn must **not** apply its older answer: invariant — a probe answer older than a published forced-reprobe result is invalidated and the hotplug rescan re-probes, so published state never moves backwards (Legacy ordering: its synchronous rescan ran before the later request) | **H70** apply a hotplug answer older than a published forced-reprobe result |
| `c0_3cii_legacy_only_reprobe_unchanged` | Legacy-only server: `GetScreenResources` runs `reprobe_connectors` synchronously (recorded seam) | **H47** route a Legacy-only reprobe through the worker |

**Hardware:** this task writes **`c0_hw_3cii_forced_reprobe_on_card1_drm`**
(written, never run by the implementer): opens card1 as Owner, drives RANDR's
forced reprobe through the new `Backend` entry with the production prober: the
reply resolves before the deadline, carries HDMI-2 with the same mode list the
synchronous `probe_connectors` returns, and no executor send happens; it
records the probe's duration. No cable, no VT; the coordinator runs it × 3
after this task.

## Task 6 — the forced-reprobe differential, coverage and the final hardware run (spec §6.3, §6.5)

**Deliver:** the Legacy/Owner differential for the forced reprobe; the
`topology` writer coverage flips to proven, citing this plan's tests and naming
the deferred rows (claiming nothing for them); `docs/phase-c0-deferred-real-server-tests.md`
gains the 3c-ii rows: `DeviceRemoved` and `DeviceAddedOrReplaced` on real
hardware (a card unbound/removed and added; vkms is the intended instrument),
the real udev monitor delivering a card node `remove`/`add`, and hotplug and
forced reprobe through the assembled server on Owner (stage 5).

| Test | Scenario | Must fail under |
| --- | --- | --- |
| `c0_3cii_forced_reprobe_differential_vulkan` | Legacy vs Owner, a mode list changes and a connector appears, then `GetScreenResources`: reply bytes, events to every installed client and `lastConfigTime` equal | **H48** publish the reprobe through the hotplug-episode publication path (different notifications) |
| `c0_3cii_topology_writer_coverage_proven` | the coverage evidence reports `topology` proven, citing the tests; the deferred rows are named, not claimed | **H49** leave `topology` unproven while claiming the evidence |

**Coordinator (C), after every task on a real path:**

| After | Run | Who |
| --- | --- | --- |
| Task 1 | `c0_hw_3cii_probe_worker_on_card1_drm` × 3; `c0_hw_3c_vt_switch_on_card1_drm` × 3 | coordinator; the user from tty4 (`sh ~/vt.sh`) |
| Task 2 | `c0_hw_3c_hotplug_on_card1_drm` × 3; `c0_hw_3b` × 3 | coordinator, the user handling the cable; coordinator |
| Task 3 | `c0_hw_3c_hotplug_on_card1_drm` × 3 if production code changed | coordinator + cable |
| Task 4 | `c0_hw_3c_hotplug_on_card1_drm` × 3 (with the real-monitor assertion) | coordinator + cable |
| Task 5 | `c0_hw_3cii_forced_reprobe_on_card1_drm` × 3 | coordinator |
| Task 6 | every run above × 3 | as above |

The final run reports the measured probe durations against the 2 s deadline.

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
  fresh open. The spec's intent (off the core thread) is unchanged. Codex
  round 1 confirmed it.
- **Fd tracking (decision 3):** the spec's "incarnation fd set" does not exist
  in production; the plan tracks workers in its own per-device ledger and
  leaves `IncarnationFdSet` wiring to 3d.
- **Forced reprobe (decision 8):** on Legacy a forced reprobe never modesets;
  the plan keeps that on Owner, so `AdministrativeReprobe` is recorded but
  starts no transaction.
- **Real monitor test (Task 4):** it rides on the physical HDMI replug in the
  hardware test, because the udev library here refuses injected messages from
  an unprivileged process.
- **Forced reprobe while the VT is away (rev 6):** the rev 5 question is
  withdrawn — the Owner now probes the same way Legacy does in that state
  (the kernel gives both the cached connector state), so there is no parity
  difference and no spec exception to decide.
- **Task order (rev 3):** the acquire moves onto the worker first (Task 1),
  so the VT test runs early; the hotplug route then switches in one task.
