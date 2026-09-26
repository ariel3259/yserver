# Stage 3c-i — VT switching on the Owner

> **Implementer:** codex (model `gpt-6-luna`, reasoning effort `xhigh`; `max` from the first send-back), run with `< /dev/null`. Hard rules, restated in every prompt: **no git write commands** (the coordinator verifies and commits); of the `#[ignore]` tests run only this plan's filters, each by its own command — `c0_3ci_`, `c0_3bi_`, `c0_3bii_`, `c0_3aii_`, `c0_3a_`, `c0_2b_add_`, `c0_conv_ciii_`, `c0_conv_cii_`, `c0_conv_cfb_`, `c0_conv_cp_`, `c0_adm`, `c0_merge_` — with `--include-ignored --skip _drm` only when the prompt records the user's GPU approval; **never** `_drm` tests, `render_acceptance`, an unfiltered `--ignored`, a VT switch, or anything that performs a modeset or takes DRM master: the hardware test of Tasks 2 and 4 is **written, never run**, by the implementer; no deletes outside the worktree; remove temporary instrumentation before finishing; never edit `docs/status.md`. **You write the implementation and the tests**; this plan gives the interfaces, the invariants, the named tests with the scenario each must exercise, and the mutations each must catch. Execute tasks in order, one at a time; stop with the tree dirty after each task. **Do not ask for approval inside a run** — if the plan leaves a real design choice open, or something it states does not hold in the code or in C.0, stop and report it (F8); never silently substitute a test shape, never weaken an existing assertion.

**Revision 2 (2026-09-26, coordinator)** — codex round 1 (2 blocking, 2 major,
all confirmed, `../findings/2026-09-26-stage-3c-i-plan-review-round1.md`): a
per-device acquire probe (B-1); the hand-off and the unknown-outcome closure
co-delivered, the mixed-server scoping moved into Task 1, the urgent
withdrawal its own task before them, 8 tasks (B-2); the `AcquireEpisode`
owner (M-1); core-level gate tests for the urgent withdrawal (M-2).

**Revision 5 (2026-09-26, coordinator)** — codex round 4 (1 blocking, 2 major,
confirmed, `../findings/2026-09-26-stage-3c-i-plan-review-round4.md`): a
deadline resolves a client token `Failed` through the ordinary ready-token
path, so the parked request is answered (B-1); the core filters every
publication against withdrawn ids, including ones queued before the
withdrawal (M-1); the driver gains a state-carrying variant used by every test
that asserts client bytes (M-2).

**Revision 4 (2026-09-26, coordinator)** — codex round 3 (2 blocking, 0 major,
confirmed, `../findings/2026-09-26-stage-3c-i-plan-review-round3.md`): Task 2's
interim acquire leaves Owner devices closed and resumes only the Legacy ones
(B-1); the acquire episode begins at the start of `on_vt_acquire`, the core
reserves its turn before draining pending requests, and the scoped Legacy
resume stages its changes for `EpisodeEnd` (B-2).

**Revision 3 (2026-09-26, coordinator)** — codex round 2 (2 blocking, 3 major,
all confirmed, `../findings/2026-09-26-stage-3c-i-plan-review-round2.md`): the
whole release is one task and the whole acquire another, so no tree exposes a
half route (B-1, M-2); any commit occupying the slot at the deadline is closed,
a client token resolves `Failed` and cannot publish late (B-2); the core gets an
episode turn (`EpisodeBegin`/`EpisodeEnd`) the `AcquireEpisode` uses (M-1); the
hardware test is written in the release task and extended in the acquire task
(M-3). Six tasks.

**Goal:** a VT switch on an Owner device is a `VTRelease`/`VTAcquire` lifecycle
transition executed through the executor, with prompt obligations that never
wait on it, a bounded hand-off, and a from-scratch reinstall on return.

**Spec:** `docs/superpowers/specs/2026-09-26-phase-c0-stage-3c-vt-and-hotplug-design.md`
revision 5 — §3 (all), §5 exceptions 1, 3, 4, 5, §6 (VT half). Umbrella:
`2026-09-22-phase-c0-stage-3-lifecycle-design.md` §3 "3c", §4, §5. C.0 §10
(state table; `CompletionUnknown` continuation, VT-release row) and `REC-4`.

## Global constraints

- The release hand-off bound is **1 s, absolute**, taken at the release signal.
- Production stays `Legacy`; a Legacy-only server runs today's
  `on_vt_release`/`on_vt_acquire` code unchanged (a test pins it).
- The client contract is parity with Legacy; the only differences are the
  spec's §5 exceptions.
- The cursor plane detach is stage 4's; the release commit is `ACTIVE=0` only.
- Every scenario test obeys the standing rules:
  **(A)** it ends with `c0_3bi_assert_end_state` (or its successor) stating its
  expected live set explicitly; **(B)** the backend advances only through the
  existing core-entry driver (`c0_3bi_core_driver_until` /
  `_before_crtc_result`) and executor replies go through the executor
  (`StubBehaviour::AcceptKernelCalls` or scripted replies), never injected into
  the Owner alone — a stub that cannot reproduce a kernel side effect says
  which in a comment; **(C)** the coordinator runs hardware after every task
  that touches a real path. No new harness (plan 3b-i-2 rev 4).

## Design decisions this plan fixes

1. Test names start with `c0_3ci_`; Vulkan tests end in `_vulkan` and use the
   3b fixtures (`owner_live_fixture`, the two-Owner position fixture, the
   mixed Legacy+Owner fixture of 3b-i-2 Task 4).
2. The VT entries are production: `Backend::on_vt_release` and
   `Backend::on_vt_acquire` (`render/backend.rs`). Tests enter through them.
   The VT ioctls (`VT_RELDISP`, `drmDropMaster`, `drmSetMaster`) sit behind
   the existing console guard / device seam; in fixtures that seam records the
   calls and their order (it is `/dev/null`-backed), and the test says so.
3. The coordinator's **`set_seat_target`** (not `observe_seat_target`, which is
   the read-only Legacy feed of 3a-ii) is how a VT switch enters the arbiter on
   a server with an Owner device; the arbiter already emits
   `LifecycleAction::ReleaseSeat` and `WithdrawOutputs`, which the driver
   (`render/admission.rs`, the `LifecycleAction` match) ignores today — this
   plan gives them execution.
4. 3c-i's acquire probe is the existing **synchronous** combined probe
   (`probe_connector_snapshot` over every device); 3c-ii moves it off the core
   thread. Tests needing an unanswered probe live in 3c-ii.
5. The **urgent withdrawal publication** (spec §4.1 rev 5) is built here,
   because an unknown release needs it first; 3c-ii reuses it.
6. Spec §6.1 tests that move between the two plans: `c0_3ci_acquire_skips_a_removed_device`
   and `c0_3ci_acquire_probe_no_reply_withdraws_owner_device` go to **3c-ii**
   (they need device removal and the asynchronous probe);
   `urgent_withdrawal_hides_unpublished_modeset` comes **here** as
   `c0_3ci_core_withdrawal_only_subtracts` (Task 1), because the mechanism
   lands here. 3c-ii's plan lists the first two.

## Review Focus

1. A release while a client modeset is dispatched on the same device, with
   a client reading `GetScreenResources` in between (Tasks 1, 2).
2. A release commit that never answers — the helper stuck in the ioctl —
   and a later VT return (Task 2 `c0_3ci_unanswered_client_modeset_at_the_deadline_vulkan`).
3. Ctrl-Alt-F2, F1, F2 in quick succession (Task 5).
4. A two-GPU box where one Owner device's acquire probe fails (Task 4
   `c0_3ci_acquire_mixed_success_vulkan`).
5. A mixed server whose Legacy suspend runs long (Task 2).

---

## Task 1 — the urgent withdrawal and the topology-episode turn in the core (spec §4.1 rev 3–5)

*(Rev 3: rev 2's Task 2 plus the episode's gate contract, M-1.)* Two core
mechanisms, each with a backend emitter; neither is raised by a production VT
path until Tasks 2 and 4, so this task leaves every route unchanged.

**Deliver — urgent withdrawal** (yserver-core + backend): a new requester-less
publication kind that the core publishes at once, **bypassing the
`RandrMutationGate` FIFO** even while an install-capable mutation holds it, and
that only **subtracts** the named outputs and CRTCs from the **published**
projection and emits the notifications for that removal — never rebuilt from
the backend model. Every other publication keeps being built from the backend
model. The driver executes `LifecycleAction::WithdrawOutputs` by withdrawing
the device's outputs from the backend model and emitting one urgent withdrawal.

*(Rev 5, M-1.)* **Publications already queued.** A requester-less publication
queued behind the gate before the withdrawal carries a deferred state-update
closure that may have captured the old projection. The core keeps the set of
withdrawn output and CRTC ids and **filters every later publication's
resulting projection against it** (after its `update_state` runs, before its
notifications), until the backend's model no longer names them — so no
publication, queued before or after the withdrawal, can restore a withdrawn
output.

**Deliver — the episode turn** (yserver-core + backend): the backend can
signal `EpisodeBegin(id)` and `EpisodeEnd(id, Option<RequesterlessPublication>)`
through a drain the core already polls (next to
`drain_requesterless_publications`). While an episode is open it **holds the
gate turn** like an install-capable member: client mutations queue behind it,
queries read the published state; `EpisodeEnd` publishes its publication (if
any) in that turn and releases it; an `EpisodeEnd` with `None` releases it
without publishing (abort). An urgent withdrawal still bypasses an open
episode. One episode at a time; a second `EpisodeBegin` while one is open is a
contract violation (debug assertion + error log).

Test layers (M-2 of round 1): **core** tests use the existing yserver-core
core-loop gate tests (`c0_3bii_` in `core_loop/run.rs`) — a real gate, a
retained `ServerState`, client reads; **backend** tests use the core-entry
driver.

*(Rev 5, M-2.)* **The driver carries the entry's state.** The existing
core-entry driver builds a fresh `ServerState` per call, so it loses the
clients and bytes of the entry that started the scenario. This task adds one
variant, `c0_3bi_core_driver_until_with_state(backend, &mut state, …)`, with the
same loop and the caller's `ServerState` (the existing function delegates to it
with a fresh state) — an extension of the one driver, not a new harness.
**Every backend test in this plan that asserts client bytes, a reply or a
publication uses it**, with the clients installed by the existing
`c0_3aii_install_dpms_core_client` helper; the gate-ordering properties stay
in the core layer.

| Test | Scenario | Must fail under |
| --- | --- | --- |
| `c0_3ci_core_withdrawal_bypasses_the_gate` (yserver-core) | the gate holds an install-capable mutation; an urgent withdrawal arrives: the listener receives the withdrawal's notifications before the mutation's reply | **V11** queue the withdrawal behind the gate |
| `c0_3ci_core_withdrawal_only_subtracts` (yserver-core) | the backend model already holds a promoted-but-unpublished change; a client reads between the withdrawal and that change's publication: it sees the withdrawal and not the change; after the publication, both | **V10** build the withdrawal from the backend model |
| `c0_3ci_core_queued_publication_cannot_restore_withdrawn` (yserver-core) | a requester-less publication that still names output X is queued behind a held gate; X is urgently withdrawn; the gate releases and the queued publication runs: X stays absent from the projection and no notification re-adds it | **V35** run queued publications unfiltered |
| `c0_3ci_core_episode_holds_the_turn` (yserver-core) | an episode is open; a client `SetCrtcConfig` arrives: it is not dispatched until `EpisodeEnd`; a query is answered at once from the published state | **V29a** let a client mutation take the turn during an episode |
| `c0_3ci_core_episode_abort_releases_the_turn` (yserver-core) | `EpisodeEnd(None)`: nothing published, the queued mutation proceeds | **V29b** publish on abort |
| `c0_3ci_withdraw_outputs_emits_urgent_withdrawal_vulkan` | the driver executes `WithdrawOutputs` for an Owner device: its outputs leave the backend model and exactly one urgent withdrawal naming them is emitted | **V27** emit an ordinary requester-less publication |

## Task 2 — the release (spec §3.1, all steps)

*(Rev 3, B-1/B-2: rev 2's Tasks 1 and 3 co-delivered — the Owner release route
switches on only with its commit, hand-off and outcome closure.)*

**Deliver — entry and prompt obligations:** on a server with at least one
Owner device, `on_vt_release` calls the coordinator's
`set_seat_target(Released)` and queues the dispatches through the lifecycle
driver. The driver executes `LifecycleAction::ReleaseSeat` as the **prompt
obligations**, synchronously, before any executor reply can be observed and
regardless of a dispatched commit in the device slot: pause input; synthesize
held key/button releases; DPMS protocol level 0 and `last_activity` reset (as
`run_suspend`); close admission on every Owner device (Presents answered
through 3a's per-CRTC blackout); pause the resource service's serviced-time
budget. `VtState` becomes `Suspending`. A Legacy-only server keeps today's
path byte for byte, and no Owner device ever receives a Legacy write.

**Deliver — the commit:** per Owner device, one lifecycle commit with 3a's
DPMS-off shape: `ACTIVE=0` on every lit CRTC, MODE_ID and the primary plane
untouched, every turned-off CRTC in `ExpectedCompletionCrtcs` with a required
out-fence; queued behind a dispatched commit (no preemption); none for a
poisoned or already-dark device. The Owner commits are dispatched **before**
the scoped Legacy suspend of a mixed server.

**Deliver — the mixed server:** the Legacy devices run today's suspend
**scoped** to them (`_for_devices` helpers); every bounded wait inside it (its
DRM-event drain, GPU idle) is capped at the **remaining** budget; a blocking
Legacy ioctl is the named limitation (spec §5.5) and nothing tries to bound it.

**Deliver — the hand-off:** the release deadline (release signal + 1 s,
absolute) is a `next_wakeup` deadline. The hand-off — `drmDropMaster` on every
device, then `VT_RELDISP(1)`, then `VtState::Suspended` — happens at the first
of "every Owner device's slot is terminal" and the deadline, from the
completion path or the wakeup, never by polling.

**Deliver — the outcomes, per Owner device, at the hand-off:**
- *Known* (every commit the slot held reached a known terminal result —
  completed, or rejected with known completion): the incarnation stays
  healthy.
- *Unknown* — *(rev 3, B-2)* **any** commit in the device's slot not known
  terminal at the deadline, whether the release commit, a queued release
  behind it, a client modeset, a composed flip or a direct commit — or
  `CompletionUnknown`, or the executor dead: C.0 §10 VT-release row —
  `StopAliasCreation`, request executor/helper termination, record the
  `REC-6` invalidation, retain any unreaped executor and the complete fd set
  in `ExecutorStalled`, quarantine; **every** outstanding piece of work on that
  device is terminalized in the same step: a queued release commit is
  cancelled unsent; a dispatched **client modeset's CRTC token resolves
  `Failed`** *(rev 5, B-1)* through the **ordinary ready-token path** — the
  backend queues the token as ready with a `ClientModesetFailure` result, and
  the core's existing `drain_ready_crtc_configs` path takes the gate
  publication, publishes nothing for a failed result (3b-ii
  `c0_3bii_failed_publishes_nothing`), replies `Failed` to the parked
  requester and releases the gate; the gate publication is **not** removed
  out of band (that would drop the reply); the executor's late completion for
  that commit is discarded by the backend (dead epoch, token already
  finished); Presents are answered by blackout. The
  incarnation is **closed forever**; `WithdrawOutputs` (Task 1) withdraws its
  outputs.
- Late results of the dead epoch change nothing.

**Interim acquire (until Task 4)** *(rev 4, B-1)*: on a server with an Owner
device, `on_vt_acquire` takes master, runs today's resume **scoped to the
Legacy devices** (`_for_devices` helpers), resumes input, and leaves **every
Owner device closed** — admission stays closed, no Legacy write and no
lifecycle commit reaches it (it stays dark) until Task 4 delivers the
reinstall. A closed incarnation stays withdrawn. A Legacy-only server keeps
today's acquire unchanged.

| Test | Scenario | Must fail under |
| --- | --- | --- |
| `c0_3ci_prompt_obligations_never_wait_vulkan` | an Owner device with a **dispatched** client modeset and no executor reply; `on_vt_release`: input paused, held keys released, seat `Released`, admission closed, a Present answered by blackout — all before the executor answers | **V1** gate the prompt obligations on the device slot being idle |
| `c0_3ci_legacy_only_release_unchanged` | a Legacy-only server: the call sequence of `on_vt_release` equals today's (recorded seam) | **V2** route a Legacy-only server through `set_seat_target` |
| `c0_3ci_release_commit_is_active_off_vulkan` | two lit outputs: one commit, `ACTIVE=0` on both CRTCs, no MODE_ID or plane property | **V3** add the primary plane detach |
| `c0_3ci_release_commit_waits_behind_dispatched_modeset_vulkan` | the client modeset completes at 50 ms: the release commit is sent after it, hand-off after the release commit completes | **V4** send the release commit while the slot is occupied |
| `c0_3ci_dark_device_contributes_no_commit_vulkan` | DPMS already off: no release commit; hand-off at once | **V5** send `ACTIVE=0` to dark CRTCs |
| `c0_3ci_release_hands_off_after_commit_vulkan` | the commit completes at 100 ms: drop master + `VT_RELDISP(1)` right after, in that order | **V6** send `VT_RELDISP` before the slot is terminal |
| `c0_3ci_release_hands_off_at_the_bound_vulkan` | the executor never answers: hand-off at exactly the deadline through `next_wakeup` | **V7** do not arm the deadline as a wakeup |
| `c0_3ci_unknown_release_closes_the_incarnation_vulkan` | the release commit is in flight at the deadline: helper termination requested, `ExecutorStalled`, outputs withdrawn by one urgent withdrawal; a later acquire submits nothing to the old incarnation | **V8** submit to the old incarnation at acquire |
| `c0_3ci_unanswered_client_modeset_at_the_deadline_vulkan` | through `on_vt_release`: a dispatched client modeset never answers; at the deadline its requester receives `Failed`, the queued release commit was never sent, the device is closed and withdrawn, and a late modeset completion publishes nothing | **V30** leave the client token pending past the hand-off |
| `c0_3ci_core_deadline_failure_replies_to_the_parked_request` (yserver-core) | a parked `SetCrtcConfig` whose token the backend resolves `Failed` at a deadline: the requester receives `Failed`, nothing is published, the gate is released and the next mutation proceeds | **V34** remove the gate publication instead of resolving the token |
| `c0_3ci_late_release_result_changes_nothing_vulkan` | the release commit's completion arrives after the hand-off: installed state, pools and publication unchanged | **V9** apply the late completion |
| `c0_3ci_interim_acquire_leaves_owner_closed_vulkan` | release then acquire before Task 4's route: the Owner device receives no ioctl and no commit, its admission stays closed; the Legacy device of a mixed server resumes | **V31** run today's unscoped resume on the Owner device |
| `c0_3ci_mixed_server_release_scopes_legacy_vulkan` | Legacy + Owner: the Legacy all-off touches only the Legacy device; the Owner device receives only its release commit | **V24** run the unscoped Legacy suspend |
| `c0_3ci_mixed_release_legacy_suspend_consumes_the_budget_vulkan` | the Legacy drain uses its whole allowance (a fixture seam on the drain's clock): the hand-off still happens at the absolute deadline | **V25** give the Legacy drain a fixed 1 s |

**Hardware (M-3):** this task writes **`c0_hw_3c_vt_switch_on_card1_drm`**
(never run by the implementer): it arms `VT_PROCESS` on its controlling tty
(the user runs it from a text VT; it skips with a logged reason otherwise),
opens card1 as Owner, composes, then × 4 switches to another VT with
`VT_ACTIVATE` and back, delivering SIGUSR1/SIGUSR2 to the production
`on_vt_release`/`on_vt_acquire`. At this task it asserts the **release**: the
hand-off happens inside the bound with the commit terminal and the device
healthy; after each return (the interim acquire) it asserts only that **no
write reached card1 outside the executor** and the device stays closed —
it does not expect a frame until Task 4. The coordinator runs it × 3 after
this task.

## Task 3 — while released (spec §3.2)

**Deliver:** `RRSetCrtcConfig` answers `Failed` (`SeatReleased`, existing);
`DPMSForceLevel` updates the protocol level, hardware
`Deferred(SeatReleased)`; connector hotplug edges are recorded, not probed;
lifecycle events below `VTRelease` in precedence are `Deferred(SeatReleased)`
and coalesce by `REC-5`. (`Shutdown` and `DeviceRemoved` are never deferred —
`DeviceRemoved`'s execution is 3c-ii's.)

| Test | Scenario | Must fail under |
| --- | --- | --- |
| `c0_3ci_requests_while_released_match_legacy_vulkan` | Legacy vs Owner, while released: `SetCrtcConfig` bytes, `DPMSForceLevel` + `DPMSInfo` bytes, no executor send on Owner | **V12** dispatch a request to KMS while released |
| `c0_3ci_dpms_while_released_is_deferred_vulkan` | DPMS off while released: the arbiter records `Deferred(SeatReleased)` with the newest level; no executor send | **V13** drop the deferred DPMS level |

## Task 4 — the acquire (spec §3.3, all steps)

*(Rev 3, M-2: rev 2's Tasks 5 and 6 co-delivered — the Owner acquire route
switches on only with its reinstall.)*

**Deliver — entry, probe, order:** `on_vt_acquire` on a server with an Owner
device: `VT_ACKACQ`; bounded `drmSetMaster` over the devices still present (a
present device that fails keeps today's exit); a synchronous **per-device**
probe — a new platform entry returning one result per open device keyed by
`DrmDeviceKey` (`Ok(snapshot)` or `Err`), probing every device even after one
fails (today's `probe_connector_snapshot` keeps its behaviour for the
Legacy-only path); then, in order: scoped Legacy resume for the Legacy devices
(today's `run_resume` steps through `_for_devices` helpers); the
per-participant dispositions of spec §3.3's table — a healthy Owner device with
a good probe starts its reinstall; an Owner device whose probe failed is closed
and **urgently withdrawn** (Task 1); a Legacy device whose probe failed keeps
today's exit; a closed incarnation stays withdrawn; then `VtState::Active`,
input resumes, xkb resyncs — **not** waiting for any Owner commit.
`set_seat_target(Owned)` is the arbiter entry; the interim acquire of Task 2
is removed.

**Deliver — the reinstall:** per healthy Owner device, one `ALLOW_MODESET`
lifecycle commit that installs the desired topology from scratch with 3b's
execution (fresh pools prepared, infallible promotion, retired bundles for
what it displaces); the DPMS projection from the coordinator's current level
(an `off` level — including one set while released — installs `ACTIVE=0`);
clocks needed only for old-active CRTCs (none after a release, so no clock
wait); the new epoch's clock probe starts after it. At `Applied`: full damage
on every output, admission reopens. A direct frame current at release does
not survive.

**Deliver — the episode:** *(rev 4, B-2)* an `AcquireEpisode` record in the
backend, created at the **start** of `on_vt_acquire` whenever the server has an
Owner device — **before** the probe and the scoped Legacy resume. It signals
`EpisodeBegin(id)` (Task 1) first, and **the core consumes that signal right
after the VT entry returns and before it drains any pending client request**
(the core's `VtAcquire` handling in `core_loop/run.rs` reserves the turn, then
drains), so no queued mutation is dispatched ahead of the episode. The scoped
Legacy resume of a mixed server **stages** its RANDR difference into the
episode instead of publishing it (today `run_resume` publishes through
`fire_randr_changes` while applying the snapshot); the staged Legacy change
and the Owner changes are published together at `EpisodeEnd`. each participant becomes terminal on its reinstall's terminal
result (`Applied`, rejected, unknown — spec §4.1's outcome table; unknown also
closes and urgently withdraws that device); Legacy participants are terminal
when their scoped resume returns; urgently withdrawn devices leave it. When
every remaining participant is terminal it signals `EpisodeEnd(id, p)` with
one publication if the topology changed while away (Legacy `run_resume`'s
events for the same difference) and `None` otherwise. A release arriving
first ends it with `EpisodeEnd(id, None)` (Task 5).

| Test | Scenario | Must fail under |
| --- | --- | --- |
| `c0_3ci_per_device_probe_collects_every_result` | three devices, the second fails: the first and third results are returned, the second is `Err` | **V28** stop at the first error |
| `c0_3ci_mixed_server_acquire_runs_scoped_legacy_resume_vulkan` | Legacy + Owner: the Legacy device relights (recorded calls) before input resumes; the Owner reinstall starts | **V14** skip the scoped Legacy resume |
| `c0_3ci_acquire_probe_error_withdraws_owner_device_vulkan` | the Owner device's probe fails: closed and withdrawn (one urgent publication); the server continues | **V15** keep today's exit for an Owner device |
| `c0_3ci_input_resumes_before_owner_reinstall_vulkan` | the reinstall is not answered: input is already resumed and `VtState` is `Active` | **V16** resume input only at the reinstall's `Applied` |
| `c0_3ci_acquire_reinstalls_from_scratch_vulkan` | the pre-release installed state is altered in the fixture (as another master would): the reinstall's description equals the one built from the desired topology alone | **V17** build the reinstall from the pre-release installed state |
| `c0_3ci_acquire_honours_dpms_off_vulkan` | DPMS off set while released: the reinstall installs `ACTIVE=0` and no frame is admitted | **V18** reinstall lit regardless of DPMS |
| `c0_3ci_acquire_mixed_success_vulkan` | two Owner devices, B's probe fails: A reinstalls and composes; B is withdrawn urgently; the episode publication covers A only | **V19** skip A's reinstall because B failed |
| `c0_3ci_acquire_episode_waits_for_every_participant_vulkan` | two healthy Owner devices, topology changed while away; A `Applied`, B held in flight: no publication and a client `SetCrtcConfig` stays queued; when B is terminal, exactly one publication, then the queued request proceeds | **V29** publish when the first participant applies |
| `c0_3ci_direct_does_not_survive_the_switch_vulkan` | direct current at release: after acquire the primary is composed; a later Present re-enters direct through eligibility | **V20** reinstall the direct buffer |
| `c0_3ci_acquire_reserves_the_turn_before_pending_requests` (yserver-core + backend) | a client `SetCrtcConfig` is pending when `VtAcquire` is handled: the core reserves the episode turn before draining it; the request is dispatched only after `EpisodeEnd` | **V32** drain pending requests before consuming `EpisodeBegin` |
| `c0_3ci_mixed_acquire_stages_legacy_changes_vulkan` | mixed server, the Legacy device's topology changed while away and the Owner reinstall is held in flight: no RANDR bytes reach clients; at `EpisodeEnd` one publication carries both changes | **V33** let the scoped Legacy resume publish directly |
| `c0_3ci_vt_switch_emits_nothing_vulkan` | release + acquire with no topology change: zero bytes to every connection, Legacy vs Owner | **V21** publish at acquire unconditionally |

**Hardware:** `c0_hw_3c_vt_switch_on_card1_drm` now also asserts each
**acquire**: the reinstall is `Applied` from scratch, a frame composes, the end
state is clean. The coordinator runs it × 3 after this task.

## Task 5 — rapid switching (spec §3.4)

**Deliver:** `VTRelease` outranks `VTAcquire`. A release arriving while an
acquire's reinstall is not yet dispatched supersedes it (`REC-4`, nothing is
sent, the episode ends with `EpisodeEnd(id, None)`); after dispatch, the
release commit waits for it inside its own 1 s bound.

| Test | Scenario | Must fail under |
| --- | --- | --- |
| `c0_3ci_release_supersedes_undispatched_acquire_vulkan` | acquire then release before the reinstall is dispatched: no reinstall commit reaches the executor; the episode aborts without publishing; hand-off as Task 2 | **V22** dispatch the superseded reinstall |
| `c0_3ci_release_after_dispatched_reinstall_vulkan` | the reinstall is dispatched: the release commit follows its terminal result, hand-off inside the bound | **V23** preempt the dispatched reinstall |

## Task 6 — coverage and the final hardware run (spec §6.3, §6.5)

**Deliver:** the `vt` writer-coverage evidence flips to proven, citing this
plan's tests and naming the deferred rows (it claims nothing for them).

| Test | Scenario | Must fail under |
| --- | --- | --- |
| `c0_3ci_vt_writer_coverage_proven` | the coverage evidence reports `vt` proven, citing the tests; the deferred rows are named, not claimed | **V26** leave `vt` unproven while claiming the evidence |

Coordinator (C): `c0_hw_3c_vt_switch_on_card1_drm` × 3 from a tty with the
user's approval after Tasks 2, 4 and 6 (each run verifies what the task added:
release; release + acquire; everything, plus `c0_hw_3b` for regression).

## Gate (every task)

`cargo +nightly fmt --check`; `cargo clippy --all-targets -- -D warnings` in
default, `--features tcp-transport`, `--features xdmcp`; each filter above
with `--include-ignored --skip _drm`; `--lib -- --skip c0_2ci --skip _drm`;
`--lib c0_2ci -- --skip _drm`; `-p yserver-core -- --skip _drm`; every
integration file except `render_acceptance` with `--skip _drm`. Report exact
counts per line. Known intermittent failures (plan 3b-i-2 rev 4, plus
`position_only_updates_in_place`): a run failing only on those is re-run once;
a repeat, or any other failure, is a finding.
