# Stage 3c-i — VT switching on the Owner

> **Implementer:** codex (model `gpt-6-luna`, reasoning effort `xhigh`; `max` from the first send-back), run with `< /dev/null`. Hard rules, restated in every prompt: **no git write commands** (the coordinator verifies and commits); of the `#[ignore]` tests run only this plan's filters, each by its own command — `c0_3ci_`, `c0_3bi_`, `c0_3bii_`, `c0_3aii_`, `c0_3a_`, `c0_2b_add_`, `c0_conv_ciii_`, `c0_conv_cii_`, `c0_conv_cfb_`, `c0_conv_cp_`, `c0_adm`, `c0_merge_` — with `--include-ignored --skip _drm` only when the prompt records the user's GPU approval; **never** `_drm` tests, `render_acceptance`, an unfiltered `--ignored`, a VT switch, or anything that performs a modeset or takes DRM master: the hardware test of Task 10 is **written, never run**, by the implementer; no deletes outside the worktree; remove temporary instrumentation before finishing; never edit `docs/status.md`. **You write the implementation and the tests**; this plan gives the interfaces, the invariants, the named tests with the scenario each must exercise, and the mutations each must catch. Execute tasks in order, one at a time; stop with the tree dirty after each task. **Do not ask for approval inside a run** — if the plan leaves a real design choice open, or something it states does not hold in the code or in C.0, stop and report it (F8); never silently substitute a test shape, never weaken an existing assertion.

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
   `urgent_withdrawal_hides_unpublished_modeset` comes **here** (Task 4) as
   `c0_3ci_urgent_withdrawal_hides_unpublished_modeset_vulkan`, because the
   mechanism lands here. 3c-ii's plan lists the first two.

## Review Focus

1. A release while a client modeset is dispatched on the same device, with
   a client reading `GetScreenResources` in between (Task 1, Task 4).
2. A release commit that never answers — the helper stuck in the ioctl —
   and a later VT return (Task 4 `c0_3ci_unknown_release_closes_the_incarnation`).
3. Ctrl-Alt-F2, F1, F2 in quick succession (Task 8).
4. A two-GPU box where one Owner device's acquire probe fails (Task 7
   `c0_3ci_acquire_mixed_success_vulkan`).
5. A mixed server whose Legacy suspend runs long (Task 9).

---

## Task 1 — `VTRelease` enters the arbiter; prompt obligations at once (spec §3.1 step 1)

**Deliver:** on a server with at least one Owner device, `on_vt_release`
calls the coordinator's `set_seat_target(Released)` and queues the
dispatches through the lifecycle driver. The driver executes
`LifecycleAction::ReleaseSeat` as the **prompt obligations**, synchronously,
before any executor reply can be observed and regardless of a dispatched
commit in the device slot: pause input; synthesize held key/button releases;
DPMS protocol level 0 and `last_activity` reset (as `run_suspend`); close
admission on every Owner device (Presents answered through 3a's per-CRTC
blackout); pause the resource service's serviced-time budget. `VtState`
becomes `Suspending`. A Legacy-only server keeps today's path byte for byte.

**Invariants:** no prompt obligation depends on an executor reply or on the
device slot; the Legacy-only path is unchanged.

| Test | Scenario | Must fail under |
| --- | --- | --- |
| `c0_3ci_prompt_obligations_never_wait_vulkan` | an Owner device with a **dispatched** client modeset (3b) and no executor reply; `on_vt_release`: input paused, held keys released, seat `Released` in the coordinator, admission closed, a Present answered by blackout — all before the executor answers | **V1** gate the prompt obligations on the device slot being idle |
| `c0_3ci_legacy_only_release_unchanged` | a Legacy-only server: the call sequence of `on_vt_release` equals today's (recorded seam) | **V2** route a Legacy-only server through `set_seat_target` |

## Task 2 — the release commit (spec §3.1 step 2)

**Deliver:** per Owner device, the `VTRelease` transition's physical step is
one lifecycle commit with 3a's DPMS-off shape: `ACTIVE=0` on every lit CRTC,
MODE_ID and the primary plane untouched, every turned-off CRTC in
`ExpectedCompletionCrtcs` with a required out-fence. When the slot holds a
dispatched commit, the release commit is queued behind it (no preemption). A
poisoned or already-dark device issues none.

| Test | Scenario | Must fail under |
| --- | --- | --- |
| `c0_3ci_release_commit_is_active_off_vulkan` | two lit outputs: the executor receives one commit, `ACTIVE=0` on both CRTCs, no MODE_ID or plane property | **V3** add the primary plane detach |
| `c0_3ci_release_commit_waits_behind_dispatched_modeset_vulkan` | release while a client modeset is dispatched: the release commit is sent only after the modeset's terminal result | **V4** send the release commit while the slot is occupied |
| `c0_3ci_dark_device_contributes_no_commit_vulkan` | DPMS already off: no release commit | **V5** send `ACTIVE=0` to dark CRTCs |

## Task 3 — the bounded hand-off (spec §3.1 step 4)

**Deliver:** the release deadline (release signal + 1 s) is armed as a
`next_wakeup` deadline. The hand-off — `drmDropMaster` on every device, then
`VT_RELDISP(1)`, then `VtState::Suspended` — happens at the first of "every
Owner device's release commit is terminal" and the deadline, from the
completion path or the wakeup, never by polling.

| Test | Scenario | Must fail under |
| --- | --- | --- |
| `c0_3ci_release_hands_off_after_commit_vulkan` | the commit completes at 100 ms (driver time): drop master + `VT_RELDISP(1)` right after, in that order | **V6** send `VT_RELDISP` before the commit's terminal result |
| `c0_3ci_release_hands_off_at_the_bound_vulkan` | the executor never answers: hand-off at exactly the deadline through `next_wakeup` | **V7** do not arm the deadline as a wakeup |

## Task 4 — release outcomes; urgent withdrawal (spec §3.1 step 5, §4.1 rev 5)

**Deliver:**
- *Known outcome* (completed, or rejected with known completion): the
  incarnation stays healthy; acquire reinstalls on it.
- *Unknown outcome* (`CompletionUnknown`, in flight at the deadline, executor
  dead): C.0 §10 VT-release row — `StopAliasCreation`, request executor/helper
  termination, record the `REC-6` invalidation, retain any unreaped executor
  and the complete fd set in `ExecutorStalled`, quarantine; the incarnation is
  closed forever; the driver executes `WithdrawOutputs` as an **urgent
  withdrawal**.
- **Urgent withdrawal publication** (yserver-core): a new requester-less
  publication kind that bypasses the `RandrMutationGate` FIFO even while an
  install-capable mutation holds it, and that only **subtracts** the device's
  outputs and CRTCs from the published projection and emits the notifications
  for that removal. Every other publication keeps being built from the
  backend model (which already excludes the withdrawn device).
- Late results of the dead epoch change nothing (3a/3b rule).

| Test | Scenario | Must fail under |
| --- | --- | --- |
| `c0_3ci_unknown_release_closes_the_incarnation_vulkan` | the release commit is in flight at the deadline: helper termination requested, `ExecutorStalled`, outputs withdrawn and published once; after a later acquire, no submission reaches the old incarnation | **V8** reinstall on the old incarnation at acquire |
| `c0_3ci_late_release_result_changes_nothing_vulkan` | the release commit's completion arrives after the hand-off: installed state, pools and publication unchanged | **V9** apply the late completion |
| `c0_3ci_urgent_withdrawal_hides_unpublished_modeset_vulkan` | two Owner devices; A's client modeset is promoted but not yet published; B's release is unknown: a client reading between B's withdrawal and A's publication sees B withdrawn and A unchanged; after A's publication, both | **V10** build the withdrawal publication from the backend model |
| `c0_3ci_withdrawal_bypasses_the_gate_vulkan` | an unrelated install-capable mutation holds the gate: the withdrawal reaches the listener before that mutation's reply | **V11** queue the withdrawal behind the gate |

## Task 5 — while released (spec §3.2)

**Deliver:** `RRSetCrtcConfig` answers `Failed` (`SeatReleased`, existing);
`DPMSForceLevel` updates the protocol level, hardware
`Deferred(SeatReleased)`, applied at acquire; connector hotplug edges are
recorded, not probed; lifecycle events below `VTRelease` in precedence are
`Deferred(SeatReleased)` and coalesce by `REC-5`. (`Shutdown` and
`DeviceRemoved` are never deferred — `DeviceRemoved`'s execution is 3c-ii's.)

| Test | Scenario | Must fail under |
| --- | --- | --- |
| `c0_3ci_requests_while_released_match_legacy_vulkan` | Legacy vs Owner, while released: `SetCrtcConfig` bytes, `DPMSForceLevel` + `DPMSInfo` bytes, no executor send on Owner | **V12** dispatch a request to KMS while released |
| `c0_3ci_dpms_while_released_applies_at_acquire_vulkan` | DPMS off while released, then acquire: the reinstall installs `ACTIVE=0` | **V13** drop the deferred DPMS level |

## Task 6 — acquire: master, probe, order, input (spec §3.3 opening, rev 3–4)

**Deliver:** `on_vt_acquire` on a server with an Owner device: `VT_ACKACQ`;
bounded `drmSetMaster` over the devices still present (a present device that
fails keeps today's exit); the synchronous combined probe over every device;
then, in this order: scoped Legacy resume for the Legacy devices (today's
`run_resume` steps through `_for_devices` helpers); the per-participant
dispositions of spec §3.3's table (a healthy Owner device with a good probe
starts its `VTAcquire` transition — Task 7; an Owner device whose probe
failed is closed and **urgently withdrawn** with Task 4's publication; a
Legacy device whose probe failed keeps today's exit); then `VtState::Active`,
input resumes, xkb resyncs — **not** waiting for any Owner commit.
`set_seat_target(Owned)` is the arbiter entry.

| Test | Scenario | Must fail under |
| --- | --- | --- |
| `c0_3ci_mixed_server_acquire_runs_scoped_legacy_resume_vulkan` | Legacy + Owner: the Legacy device relights (recorded calls) before input resumes; the Owner device's reinstall starts | **V14** skip the scoped Legacy resume |
| `c0_3ci_acquire_probe_error_withdraws_owner_device_vulkan` | the Owner device's probe fails: it is closed and withdrawn (one urgent publication); the server continues | **V15** keep today's exit for an Owner device |
| `c0_3ci_input_resumes_before_owner_reinstall_vulkan` | the reinstall commit is not answered: input is already resumed and `VtState` is `Active` | **V16** resume input only at the reinstall's `Applied` |

## Task 7 — the reinstall (spec §3.3 steps 3–6)

**Deliver:** per healthy Owner device, one `ALLOW_MODESET` lifecycle commit
that installs the desired topology from scratch with 3b's execution (fresh
pools prepared, infallible promotion, retired bundles for what it displaces);
the DPMS projection from the coordinator's current level; clocks needed only
for old-active CRTCs (none after a release, so no clock wait); the new epoch's
clock probe starts after it (3a carried item). At `Applied`: full damage on
every output, admission reopens. A direct frame current at release does not
survive (composed primaries installed). If the topology changed while away,
one publication for the acquire episode, requester-less, with Legacy
`run_resume`'s events for the same difference; no publication otherwise.

| Test | Scenario | Must fail under |
| --- | --- | --- |
| `c0_3ci_acquire_reinstalls_from_scratch_vulkan` | the pre-release installed state is altered in the fixture (as another master would): the reinstall's commit description is identical to the one built from the desired topology alone | **V17** build the reinstall from the pre-release installed state |
| `c0_3ci_acquire_honours_dpms_off_vulkan` | protocol DPMS off at acquire: the reinstall installs `ACTIVE=0` and no frame is admitted | **V18** reinstall lit regardless of DPMS |
| `c0_3ci_acquire_mixed_success_vulkan` | two Owner devices, B's probe fails: A reinstalls and composes; B is withdrawn urgently; the acquire publication covers A only | **V19** skip A's reinstall because B failed |
| `c0_3ci_direct_does_not_survive_the_switch_vulkan` | direct current at release: after acquire the primary is composed; a later Present re-enters direct through eligibility | **V20** reinstall the direct buffer |
| `c0_3ci_vt_switch_emits_nothing_vulkan` | release + acquire with no topology change: zero bytes to every connection, Legacy vs Owner | **V21** publish at acquire unconditionally |

## Task 8 — rapid switching (spec §3.4)

**Deliver:** `VTRelease` outranks `VTAcquire`. A release arriving while an
acquire's reinstall is not yet dispatched supersedes it (`REC-4`, nothing is
sent); after dispatch, the release commit waits for it inside its own 1 s
bound.

| Test | Scenario | Must fail under |
| --- | --- | --- |
| `c0_3ci_release_supersedes_undispatched_acquire_vulkan` | acquire then release before the reinstall is dispatched: no reinstall commit reaches the executor; hand-off as Task 3 | **V22** dispatch the superseded reinstall |
| `c0_3ci_release_after_dispatched_reinstall_vulkan` | the reinstall is dispatched: the release commit follows its terminal result, hand-off inside the bound | **V23** preempt the dispatched reinstall |

## Task 9 — mixed-server release (spec §3.1 step 3, rev 4–5)

**Deliver:** on a mixed server the Owner release commits are dispatched
**first**, then the scoped Legacy suspend runs on the core with every bounded
wait (its DRM-event drain, GPU idle) capped at the **remaining** budget of the
absolute deadline; then the delivered Owner completions are drained and the
hand-off happens at the first of "all Owner terminal" and the deadline. A
blocking Legacy KMS ioctl is the named limitation (spec §5.5): no code tries
to bound it.

| Test | Scenario | Must fail under |
| --- | --- | --- |
| `c0_3ci_mixed_server_release_scopes_legacy_vulkan` | Legacy + Owner: the Legacy all-off touches only the Legacy device; the Owner device receives only its release commit | **V24** run the unscoped Legacy suspend |
| `c0_3ci_mixed_release_legacy_suspend_consumes_the_budget_vulkan` | the Legacy drain is made to use its whole allowance (a fixture seam on the drain's clock): the hand-off still happens at the absolute deadline | **V25** give the Legacy drain a fixed 1 s |

## Task 10 — coverage, differential and the hardware test (spec §6.3, §6.5)

**Deliver:** the `vt` writer-coverage evidence flips to proven, citing this
plan's tests and naming the deferred rows (it claims nothing for them). The
card1 hardware test **`c0_hw_3c_vt_switch_on_card1_drm`**, written and never
run by the implementer: it arms `VT_PROCESS` on its controlling tty (the user
runs it from a text VT), opens card1 as Owner, composes, then × 4 switches to
another VT with `VT_ACTIVATE` and back, handling SIGUSR1/SIGUSR2 through the
production `on_vt_release`/`on_vt_acquire`; each release must hand off inside
the bound, each acquire must reinstall and compose; the end state is clean. It
skips with a logged reason when stdin is not a VT.

| Test | Scenario | Must fail under |
| --- | --- | --- |
| `c0_3ci_vt_writer_coverage_proven` | the coverage evidence reports `vt` proven, citing the tests; the deferred rows are named, not claimed | **V26** leave `vt` unproven while claiming the evidence |

Coordinator (C): runs `c0_hw_3c_vt_switch_on_card1_drm` × 3 from a tty with
the user's approval after Tasks 2, 3, 7 and 10.

## Gate (every task)

`cargo +nightly fmt --check`; `cargo clippy --all-targets -- -D warnings` in
default, `--features tcp-transport`, `--features xdmcp`; each filter above
with `--include-ignored --skip _drm`; `--lib -- --skip c0_2ci --skip _drm`;
`--lib c0_2ci -- --skip _drm`; `-p yserver-core -- --skip _drm`; every
integration file except `render_acceptance` with `--skip _drm`. Report exact
counts per line. Known intermittent failures (plan 3b-i-2 rev 4, plus
`position_only_updates_in_place`): a run failing only on those is re-run once;
a repeat, or any other failure, is a finding.
