# Phase C.0 stage 3c — VT switching, connector hotplug and device removal on the Owner

**Revision 1 (2026-09-26).** Umbrella:
`docs/superpowers/specs/2026-09-22-phase-c0-stage-3-lifecycle-design.md` §3
("3c — VT, hotplug and devices"), §4 (client contract), §5 (evidence regime).
Builds on 3a (arbiter, driver queue, DPMS as `ACTIVE`, blackout per CRTC) and
3b (client modeset execution, retired-output bundles, `RandrMutationGate`,
requester-less publication). One spec, **two plans**: 3c-i (VT) and 3c-ii
(connector hotplug and device removal).

Production stays `Legacy` until stage 5; every Owner path here is reached in
fixtures and in the card1 hardware tests, and the Legacy fork is unchanged
except where a mixed server scopes it (the 3a-ii / 3b-i-2 pattern).

## 1. Outcome and baseline

**Outcome.** On an Owner device, a VT switch and a connector hotplug are
lifecycle transitions of the per-device arbiter, executed through the
executor, never by an ioctl on the core thread, and never with the core thread
waiting on the executor. A client sees exactly what it sees on Legacy today.

**Baseline (Legacy, measured in the tree).**

- VT uses `VT_PROCESS` (`kms/console.rs`); the kernel completes a release only
  after `VT_RELDISP(1)`. `on_vt_release` (`render/backend.rs`, `fn on_vt_release`)
  is synchronous: pause input → `run_suspend` (synthesize held releases, DPMS
  protocol level 0, direct shadow, `wait_idle_bounded`, `cursor_plane_hide_all`,
  all-off, 1 s drain of old DRM events, `scene.drain_all`,
  `reset_scanout_bos_for_suspend`) → `drmDropMaster` on every device →
  `VT_RELDISP(1)`. Any failure calls `request_exit()`.
- `on_vt_acquire`: `VT_ACKACQ` → bounded `drmSetMaster` (10 × 5 ms) →
  `run_resume` (connector probe, quiesce if an active output vanished or direct
  is up, `apply_connector_snapshot`, registry reconcile, relayout,
  `fire_randr_changes` when the registry or active topology changed, all-on,
  gamma, cursor re-arm) → resume input → xkb resync. It publishes RANDR changes
  only when the topology changed while away.
- Hotplug: a udev monitor on subsystem `drm` (`kms/hotplug.rs`); any
  `add`/`remove`/`change` arms a 150 ms debounced `run_display_rescan`, which
  probes connectors on the already-open devices, quiesces if an active output
  vanished, relights remembered routes, relayouts and publishes. It is skipped
  while the VT is not `Active`. `reprobe_connectors` is RANDR's forced reprobe,
  run synchronously on the core thread (3b-ii counts it in the bound as
  `L_reprobe`).
- DRM devices are opened once at startup. A card that appears is never opened;
  a card that disappears only makes the probe fail (logged).
- Timestamps: Legacy hotplug publishes through
  `rebuild_randr_state(set_time: None, config_changed)` — `lastSetTime`
  preserved, `lastConfigTime` advanced only on a configuration change. The
  requester-less publication of 3b-ii
  (`yserver-core/src/backend/trait_def.rs`, `RequesterlessPublication::publish`)
  applies the same rule.

## 2. Decisions (user, 2026-09-26)

1. One spec, two plans (3c-i VT; 3c-ii hotplug and device removal).
2. **VT release is asynchronous** (option A). The history of the synchronous
   design (2026-05-27 VT spec: wlroots model, every pre-C.0 KMS write was a
   synchronous ioctl, loop stalls accepted with timeouts) does not survive
   C.0: Owner writes are executor commits, and the umbrella forbids prompt
   obligations waiting on the executor.
3. **The switch always happens** (option a): when the release commit fails,
   is unknown or misses its bound, the master is dropped and `VT_RELDISP(1)`
   sent anyway; the cost is paid at acquire (full reinstall).
4. The release bound is **1 s** (Legacy's own suspend drain bound), not the
   30 s lifecycle bootstrap deadline.
5. **Devices, option (a):** `DeviceRemoved` is executed on Owner devices;
   `DeviceAddedOrReplaced` is typed and recorded and a new card is ignored, as
   today. Hardware for both is deferred.
6. Tests that need the assembled server on Owner, or hardware this box lacks,
   go to `docs/phase-c0-deferred-real-server-tests.md`.

## 3. VT on the Owner (plan 3c-i)

### 3.1. Release

`on_vt_release` on a server with at least one Owner device:

1. **Prompt obligations, at once**, never behind the executor and never behind
   a dispatched client modeset (closes 3b's carried item: a dispatched
   modeset defers only the release *commit*, not these): pause input;
   synthesize held key/button releases; `SeatTarget::Released` into the
   coordinator; the DPMS protocol level becomes 0 (as Legacy); admission
   closes for every Owner device (frames wait; Presents are answered by 3a's
   per-CRTC blackout path); the resource service's serviced-time budget pauses
   (as today).
2. **The release commit** is a `VTRelease` lifecycle transition per Owner
   device: `ACTIVE=0` on every lit CRTC, the same commit shape as 3a's DPMS
   off (MODE_ID and the primary plane untouched, every turned-off CRTC in
   `ExpectedCompletionCrtcs`). If the device slot holds a dispatched commit
   (a client modeset, a composed flip), the release commit waits for it —
   inside the bound. The cursor plane detach is stage 4's (as for DPMS, umbrella
   carried item); with `ACTIVE=0` nothing of ours is visible.
3. **Legacy devices of a mixed server** run today's synchronous suspend,
   scoped to the Legacy devices (`_for_devices` helpers), at step 1.
4. **Hand-off.** When every Owner device's release commit has reached a
   terminal result, or the 1 s bound expires, whichever is first: `drmDropMaster`
   on every device, then `VT_RELDISP(1)`. The bound is a `next_wakeup`
   deadline, so the loop wakes for it without polling.
5. **A device whose release did not complete** (rejected, `CompletionUnknown`,
   executor dead, or still in flight at the bound) is marked
   **reinstall-required**. Its late results arrive in a dead epoch and change
   no current state (3a/3b stale-result rule); resources whose release is
   unproven stay quarantined for 3d; a dead executor makes the device
   `Poisoned` (3a) and it stays closed until 3d's recovery.
6. A poisoned or already-dark device contributes no commit; it does not
   delay the hand-off.

The VT state machine gains no new state: `Suspending` lasts until
`VT_RELDISP(1)`; `Suspended` after it.

### 3.2. While released

Parity with Legacy: `RRSetCrtcConfig` answers `Failed` (`SeatReleased`, 3b);
`DPMSForceLevel` updates the protocol level and its hardware effect is
`Deferred(SeatReleased)`; hotplug edges are recorded, not probed; any
lifecycle event targeting an Owner device is `Deferred(SeatReleased)` and
coalesces by `REC-5`.

### 3.3. Acquire

`on_vt_acquire` keeps `VT_ACKACQ` and the bounded `drmSetMaster` (a failure
keeps today's exit). Then, per Owner device, a `VTAcquire` transition:

1. A connector probe (3c-i uses the existing synchronous probe at this site;
   3c-ii moves every probe site, this one included, off the core thread).
2. The recorded hotplug edges and the probe result are applied through 3c-ii's
   topology path when it exists; in 3c-i, through the same reconcile/relayout
   logic `run_resume` uses today, logically, with no ioctl.
3. **A full reinstall**: one `ALLOW_MODESET` lifecycle commit per device that
   installs the desired topology — every lit output's mode, CRTC and composed
   primary — using 3b's execution (preparation with fresh pools, infallible
   promotion, retired bundles for what it displaces). It never assumes the
   pre-release hardware state: the other master may have changed anything.
   The DPMS projection follows the coordinator's current level (3a
   "projections follow topology"); an `off` level installs `ACTIVE=0`. Clocks
   are needed only for old-active CRTCs (3b), so a reinstall onto dark CRTCs
   waits for no clock; the new epoch's clock probe starts after it (3a carried
   item: probe at VT reacquire).
4. Full damage on every output; admission reopens when the reinstall is
   `Applied`; input resumes and xkb resyncs as today.
5. If the topology changed while away, the change is published once, as a
   requester-less publication at `Applied` (3b-ii), with the events Legacy's
   `run_resume` emits in the same case.
6. A direct frame current at release does not survive: the reinstall installs
   composed primaries; direct re-enters through ordinary eligibility.

### 3.4. Rapid switching and precedence

`VTRelease` outranks `VTAcquire` (C.0 precedence order, `LifecycleKind::ALL`).
A release arriving while an acquire's reinstall is not yet dispatched
supersedes it (`REC-4`); after dispatch, the release commit waits for it inside
its own 1 s bound. The kernel does not deliver an acquire before our
`VT_RELDISP(1)`.

### 3.5. Client contract

A VT switch with no topology change produces **no client-visible event and no
reply change** (umbrella §4 VT invariant; Legacy today, Xorg
`xf86Events.c:358`). Test: `c0_3ci_vt_switch_emits_nothing` (bytes to every
connection, Legacy vs Owner).

## 4. Connector hotplug and device removal (plan 3c-ii)

### 4.1. The probe leaves the core thread

`probe_connector_snapshot` runs on a probe worker with its own DRM fd (no
master needed for `GETRESOURCES`/`GETCONNECTOR`); its result returns to the
core as a message tagged with the device, its incarnation and a probe epoch,
and a stale result (older epoch, other incarnation) is discarded. Every probe
site uses it: the debounced udev edge, RANDR's forced reprobe, and 3c-i's
acquire. While a probe is outstanding the loop serves clients from the
published state.

RANDR's forced reprobe (`reprobe_connectors`, behind an install-capable
mutation in the gate since 3b-ii) now parks `GetScreenResources` until the
probe result arrives or its deadline expires; `L_reprobe` leaves the
synchronous term of 3b-ii's bound and becomes an `E`-like stage with its own
deadline. On expiry the reply carries the published state, as Legacy does when
its synchronous probe fails. The exact deadline is the plan's (measured on
card1, bounded above by 2 s).

### 4.2. Classification

A probe result is compared with the device's discovered topology:

| Difference | Lifecycle kind |
| --- | --- |
| connector set or connector identity (EDID) changed | `IdentityChangingHotplug` |
| only mode lists or other discovered objects changed | `TopologyRebuild` |
| the result of RANDR's forced reprobe | `AdministrativeReprobe` |

(`desired.rs` already distinguishes identity-changing from object-only
dirtiness.) An unchanged result produces no transition and no publication.

### 4.3. Applying a topology change on an Owner device

One lifecycle transaction per device, the 3b client-modeset shape:

- an active output that disappeared is disabled (its CRTC off, its scene state
  and pool retired into a bundle that waits for its KMS proof — 3b);
- a reconnected output with a remembered route is relit in the same commit;
- the layout policy (horizontal recompaction, reserved slots, framebuffer
  extent) is the existing logical code, shared with Legacy;
- a root-storage change gives every kept output full damage and an ordinary
  composed repaint, never a lifecycle commit (3b);
- the DPMS projection is refreshed first (3a);
- while the VT is released the change waits for 3.3.

### 4.4. Publication and timestamps

The change is published once, as a requester-less publication when the
transition reaches `Applied`: `lastSetTime` preserved, `lastConfigTime`
advanced only on a configuration change — the rule Legacy's
`rebuild_randr_state(None, config_changed)` applies, so this closes 3b's
carried timestamp item as a verified equality, not a new rule. Events and
`GetScreenResources` contents match Legacy for the same change. Test:
`c0_3cii_hotplug_timestamps_match_legacy`.

### 4.5. `DeviceRemoved`

A udev `remove` for the device node of an open device, or a probe answering
`ENODEV`, raises `DeviceRemoved` for that device:

- **Owner device:** no KMS call is attempted (the device is gone); the
  device's arbiter enters `Removed`; every in-flight commit terminates as
  `CompletionUnknown` and its resources are quarantined for 3d; the executor
  helper is reaped; its outputs are withdrawn logically and the change is
  published requester-less; the server continues with the other devices.
- **The renderer device** (the Vulkan device): the server cannot continue and
  exits, as it does today on device loss.
- **Legacy devices:** unchanged (the Legacy fork stays as it is until stage 5).

### 4.6. `DeviceAddedOrReplaced`

A udev `add` for a card that is not open raises `DeviceAddedOrReplaced`; it is
recorded (one log line) and ignored — the card is not opened, as today. A
removed device that reappears stays `Removed`. Hot-add is out of scope for C.0
(decision 5).

## 5. Named exceptions to Legacy parity

1. **VT switch latency:** on Owner the switch completes after the release
   commit (≤ 1 s bound), not immediately after the synchronous all-off.
   Invisible to clients.
2. **`GetScreenResources` under a forced reprobe** waits for the off-thread
   probe instead of blocking the whole loop; same reply contents.

Any other difference found by the differential is a defect.

## 6. Evidence

Every test cites a C.0 §16.1 requirement group (`REC-1..6`, `CAP`, `COMMIT`,
`ID`, `MULTI`). The standing rules apply to every scenario test: **(A)** it ends
with the end-state check (`c0_3bi_assert_end_state` or its successor) stating
its expected live set; **(B)** the backend advances only through the existing
core-entry driver (`c0_3bi_core_driver_until`) and executor replies go through
the executor (`StubBehaviour::AcceptKernelCalls` or scripted replies), never
injected into the Owner alone — a stub that cannot reproduce a kernel side
effect says which in a comment; **(C)** hardware after every task that touches
a real path, run by the coordinator. No new harness is built (plan 3b-i-2
rev 4).

### 6.1. Named tests (the plans may add, never drop)

3c-i: `c0_3ci_prompt_obligations_never_wait` (release while a client modeset
is dispatched: input paused, seat released, admission closed before any
executor reply); `c0_3ci_release_hands_off_after_commit`;
`c0_3ci_release_hands_off_at_the_bound` (commit never answered: drop master +
`VT_RELDISP(1)` at 1 s, device reinstall-required); `c0_3ci_late_release_result_changes_nothing`;
`c0_3ci_acquire_reinstalls_from_scratch` (the reinstall's description does not
depend on the pre-release installed state); `c0_3ci_acquire_honours_dpms_off`;
`c0_3ci_requests_while_released_match_legacy`; `c0_3ci_release_supersedes_undispatched_acquire`;
`c0_3ci_mixed_server_release_scopes_legacy`; `c0_3ci_vt_switch_emits_nothing`.

3c-ii: `c0_3cii_probe_runs_off_the_core_thread`; `c0_3cii_stale_probe_discarded`;
`c0_3cii_classification`; `c0_3cii_unplug_retires_and_publishes`;
`c0_3cii_replug_relights_remembered_route`; `c0_3cii_hotplug_timestamps_match_legacy`;
`c0_3cii_forced_reprobe_parks_and_expires`; `c0_3cii_hotplug_while_released_waits_for_acquire`;
`c0_3cii_device_removed_withdraws_and_continues`; `c0_3cii_device_added_is_ignored`;
the Legacy/Owner differential of backend state and client bytes for unplug,
replug and forced reprobe.

### 6.2. Mutations (criteria, the plans name them per test)

A prompt obligation that waits for an executor reply; `VT_RELDISP` before the
release commit's terminal result or the bound; the bound not armed as a
wakeup; a late release result mutating current state; the reinstall built
from the pre-release installed state; the reinstall ignoring the DPMS level;
a VT switch emitting a RANDR event; a request while released dispatched to
KMS; the probe run on the core thread; a stale probe result applied; a
hotplug publication advancing `lastSetTime`; a kept output repainted by a
lifecycle commit; a KMS call on a removed device; a removed device ending the
server when it is not the renderer; a new card opened.

### 6.3. Hardware (card1, from a tty, with the user's approval)

- 3c-i: VT switch away and back × 4 (`c0_hw_3c_vt_switch_on_card1_drm`): each
  release hands off inside the bound, each acquire reinstalls and composes,
  end state clean. The test needs a second VT to switch to and the user at the
  machine.
- 3c-ii: physical HDMI-2 unplug and replug × 4
  (`c0_hw_3c_hotplug_on_card1_drm`), done by hand while the test waits.

### 6.4. Deferred (`docs/phase-c0-deferred-real-server-tests.md`)

`DeviceRemoved` and `DeviceAddedOrReplaced` on real hardware; VT and hotplug
through the assembled server on Owner (stage 5's layer-3 rerun).

### 6.5. Exit

`vt` and `topology` writer coverage proven, citing the tests above and naming
the deferred rows; nothing claimed for them.

## 7. Plans

- **3c-i — VT** (§3, §5.1, the VT half of §6). Order: prompt obligations and
  the `VTRelease` transition; the release commit and the bounded hand-off;
  failures and late results; acquire and the reinstall; while released; rapid
  switching; mixed server; differential and hardware.
- **3c-ii — hotplug and device removal** (§4, §5.2, the rest of §6). Order:
  the probe worker; classification; applying on Owner; publication and
  timestamps; forced reprobe off the core thread; while released;
  `DeviceRemoved`; `DeviceAddedOrReplaced`; differential and hardware.

Each at most ~14 tasks; a plan that grows is split without asking.

## 8. Carried and out of scope

- **Stage 4:** the cursor plane detach in the release commit (with DPMS's).
- **3d:** recovery of a `Poisoned` device after a failed release; transfer
  of quarantined release resources; `Removed` teardown ordering.
- **Activation (4/5):** one resource service per Owner device (3b-i-2 F15).
- **Out of scope for C.0:** opening a hot-added card.

## 9. For the user

- The release commit carries `ACTIVE=0` only; the cursor detach mentioned
  while brainstorming is stage 4's, as it is for DPMS.
- 3c-i's acquire uses the synchronous probe until 3c-ii replaces every probe
  site — 3c-i can land first without waiting for the probe worker.
- The umbrella's pairing of entry points to kinds is read as the
  classification of §4.2 (by what changed), not a fixed entry-to-kind map.
