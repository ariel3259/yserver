# Phase C.0 stage 3c — VT switching, connector hotplug and device removal on the Owner

**Revision 3 (2026-09-26)** — codex round 2 (2 blocking, 2 major, all
confirmed, `../findings/2026-09-26-stage-3c-design-review-round2.md`): an
unknown release withdraws the device's outputs logically at once (B-1); every
probe episode has a deadline and a terminal disposition, and an acquire whose
Owner probe fails closes and withdraws that device instead of exiting (B-2);
a mixed server's acquire runs the scoped Legacy resume (M-1); one episode
holds one RANDR gate turn and publishes once, after every participant's
commit is terminal, with a partial-failure rule (M-2).

**Revision 2 (2026-09-26)** — codex round 1 (3 blocking, 2 major, all
confirmed, `../findings/2026-09-26-stage-3c-design-review-round1.md`): device
removal is never deferred by a released seat (B-1); an unknown or timed-out
release follows C.0 §10's VT-release row — helper termination, `ExecutorStalled`,
no reinstall on the old incarnation, the fresh incarnation is 3d's (B-2); a
forced reprobe that misses its deadline ends its gate turn, its result is
discarded and a background rebuild follows (B-3); one probe episode covers
every device with Legacy's combined failure boundary (M-1); the udev monitor
yields typed events mapped to open devices (M-2).

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
5. **Outcome per device** *(rev 2, B-2)*:
   - **Completed**, or **rejected with known completion** (the kernel refused
     the commit; nothing changed): the incarnation is healthy; acquire
     reinstalls on it (§3.3).
   - **Unknown**: `CompletionUnknown`, still in flight at the bound, or the
     executor dead. C.0 §10's VT-release row applies: stop fd-alias creation,
     request executor/helper termination, record the `REC-6` invalidation,
     retain any unreaped executor and the complete old fd set in
     `ExecutorStalled`, keep resources quarantined, and reap/close
     asynchronously. Late results arrive in a dead epoch and change no current
     state. The device is **closed**: no submission ever again on that
     incarnation. Its reinstall needs `VTAcquire` **and** the old-lease
     retirement barrier **and** a fresh KMS incarnation with qualified
     installation — creating that incarnation is 3d's (the `REC-1` machinery).
     *(Rev 3, B-1.)* `ExecutorStalled` is **logical withdrawal** (C.0 §10
     state table): the device's outputs and CRTCs leave the active
     RANDR/backend model at once, published requester-less, and stay withdrawn
     until 3d's qualified recovery. This failure publication is the one
     client-visible exception to the VT no-event invariant (§5); a test proves
     no submission reaches the old incarnation.
6. A poisoned or already-dark device contributes no commit; it does not
   delay the hand-off.

The VT state machine gains no new state: `Suspending` lasts until
`VT_RELDISP(1)`; `Suspended` after it.

### 3.2. While released

Parity with Legacy: `RRSetCrtcConfig` answers `Failed` (`SeatReleased`, 3b);
`DPMSForceLevel` updates the protocol level and its hardware effect is
`Deferred(SeatReleased)`; connector hotplug edges are recorded, not probed;
lifecycle events below `VTRelease` in precedence that target an Owner device
are `Deferred(SeatReleased)` and coalesce by `REC-5`.

*(Rev 2, B-1.)* **`Shutdown` and `DeviceRemoved` are never deferred by a
released seat** — they outrank `VTRelease` (C.0 `REC-4`). The udev monitor
keeps being drained while released; a removal is handled at once (§4.5):
logical withdrawal and a requester-less publication while the VT is away; a
removed renderer still ends the server.

### 3.3. Acquire

`on_vt_acquire` keeps `VT_ACKACQ` and the bounded `drmSetMaster` over the
devices still present; a device removed while away (§3.2) is not asked for
master and does not trigger the exit, which stays for a present device that
fails.

*(Rev 3, M-1.)* **Order on a mixed server:** after master, the acquire probe
episode (§4.1) runs over every device; then the **scoped Legacy resume** runs
synchronously for the Legacy devices (today's `run_resume` steps — relight,
gamma, cursor re-arm — through the `_for_devices` helpers); then the Owner
transitions below are started. **Input resumes and the VT state reaches
`Active` once the Legacy resume is done and the Owner transitions are
started** — input never waits for an Owner commit; Owner frames wait in
admission until their device's reinstall is `Applied`. A Legacy-only server
runs today's code unchanged.

*(Rev 3, B-2.)* **If the acquire episode fails** (a probe error, or no answer
by the episode deadline): on a Legacy-only server, today's exit; with Owner
devices, a Legacy device's failed probe keeps today's exit (Legacy is
unchanged), while an **Owner device whose probe failed is closed and
logically withdrawn** (published requester-less) and the episode continues
for the other devices — the Owner route does not end the server for one
device, as 3a decided for completion loss. The episode deadline is the probe
deadline of §4.1.

Then, per healthy Owner device (§3.1 step 5), a `VTAcquire` transition:

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
   `Applied`. (Input and the xkb resync happen at the ordering point above,
   not here.)
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
and a stale result (older epoch, other incarnation) is discarded.

*(Rev 2, M-1.)* **One probe episode covers every open device** — a hotplug
edge, a forced reprobe or an acquire probes all of them. The episode applies
nothing until every device has answered, and **if any device's probe fails the
episode applies and publishes nothing** (Legacy's combined boundary,
`run_display_rescan`/`run_resume`); the next edge starts a new episode. A probe
answering `ENODEV` is not a failure: it is that device's `DeviceRemoved`
(§4.5), and the episode continues without it. When every answer is in, the
changes are applied as per-device commits (§4.3) and published once.

*(Rev 3, B-2.)* **Every episode has a deadline** (the probe deadline below).
No answer by then counts as a failed probe; the episode's epoch is
invalidated, so a late answer is discarded. A failed hotplug or forced-reprobe
episode applies and publishes nothing (Legacy); the acquire episode's failure
disposition is §3.3's.

*(Rev 3, M-2.)* **One episode, one gate turn, one publication.** A topology
episode takes one RANDR gate turn (3b-ii ordering for requester-less
publications) from its first commit to its publication. Legacy participants
apply synchronously inside the turn; Owner participants run their per-device
transactions. The episode publishes **once, when every participant is
terminal**, bounded by the lifecycle deadline of the transactions:

| Participant outcome | What the publication shows for it |
| --- | --- |
| `Applied` | the new topology |
| rejected with known completion (nothing changed, 3b) | its previous installed topology — except outputs whose connector is physically gone, which are withdrawn logically |
| `CompletionUnknown` / stalled | logical withdrawal (`ExecutorStalled`) |

No participant's change is published before the others are terminal. Every probe
site uses it: the debounced udev edge, RANDR's forced reprobe, and 3c-i's
acquire. While a probe is outstanding the loop serves clients from the
published state.

RANDR's forced reprobe (`reprobe_connectors`, behind an install-capable
mutation in the gate since 3b-ii) now parks `GetScreenResources` until the
probe episode's result arrives or its deadline expires; `L_reprobe` leaves the
synchronous term of 3b-ii's bound and becomes an `E`-like stage with its own
deadline (the plan's, measured on card1, at most 2 s). *(Rev 2, B-3.)* The
forced reprobe **owns its gate turn** until one of:

- the result arrives: it is applied and published in the turn (as today), then
  the reply is sent and the turn ends;
- the deadline expires: the probe epoch is **invalidated** (its result, when it
  comes, is discarded unapplied — it can never change topology after a later
  mutation validated against the old state), the reply carries the published
  state, the turn ends, and a **background `TopologyRebuild` probe** is armed
  so a real change is still found and published requester-less (§4.4) in its
  own turn.

Named exception (§5): after a timeout the reply may omit a change a slower
synchronous Legacy probe would have included; the change reaches the client
by the later requester-less publication.

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

*(Rev 2, M-2.)* **The monitor yields typed events.** `DrmHotplugMonitor::drain`
returns, per uevent, the action (`add`/`remove`/`change`), the device number
(`dev_t`), the devnode and whether it is a card node, a connector sub-device or
carries `HOTPLUG=1`; today's boolean is derived from them. A classifier maps
each event to the open device by `dev_t` → `DrmDeviceKey` (and its current
incarnation): `remove` of an open card node → `DeviceRemoved`; `add` of a card
node that is not open → `DeviceAddedOrReplaced`; any `change` or connector
sub-device event of an open card → a connector edge (§4.1). Tests drive the
classifier with the typed records the monitor produces, and one test runs the
real monitor end to end on a live udev socket; a stub cannot prove the
kernel's actual delivery, which is a deferred hardware row.

A classified `remove` of an open device node, or a probe answering `ENODEV`,
raises `DeviceRemoved` for that device:

- **Owner device** (C.0 §10 device-removal row): no KMS call is attempted;
  stop fd-alias creation, request executor/helper termination, record
  `Invalidated(DeviceRemoved)`, **withdraw the outputs logically at once** and
  publish requester-less — never delayed by an unreaped helper, which stays
  `ExecutorStalled` holding its handles and quarantine until reap; every
  in-flight commit terminates as `CompletionUnknown` into quarantine for 3d;
  the arbiter enters `Removed`; the server continues with the other devices.
  This holds with the VT released (§3.2).
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
   probe instead of blocking the whole loop; same reply contents — except
   *(rev 2, B-3)* when the probe misses its deadline: the reply carries the
   published state, and the change arrives by a later requester-less
   publication.
3. *(Rev 3.)* **Failure paths publish withdrawals where Legacy exits:** an
   unknown release (§3.1) or a failed Owner acquire probe (§3.3) withdraws
   that device's outputs with a requester-less publication, where Legacy ends
   the server; the ordinary VT switch still emits nothing.
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
`c0_3ci_mixed_server_release_scopes_legacy`; `c0_3ci_vt_switch_emits_nothing`;
*(rev 3)* `c0_3ci_mixed_server_acquire_runs_scoped_legacy_resume`,
`c0_3ci_acquire_probe_error_withdraws_owner_device`,
`c0_3ci_acquire_probe_no_reply_withdraws_owner_device` (both through the acquire
entry), `c0_3ci_input_resumes_before_owner_reinstall`;
*(rev 2)* `c0_3ci_unknown_release_closes_the_incarnation` (now also: its
outputs are withdrawn and published once) (a release commit
still in flight at the bound: helper termination requested, `ExecutorStalled`,
and after acquire no submission reaches the old incarnation) and
`c0_3ci_acquire_skips_a_removed_device`.

3c-ii: `c0_3cii_probe_runs_off_the_core_thread`; `c0_3cii_stale_probe_discarded`;
`c0_3cii_classification`; `c0_3cii_unplug_retires_and_publishes`;
`c0_3cii_replug_relights_remembered_route`; `c0_3cii_hotplug_timestamps_match_legacy`;
`c0_3cii_forced_reprobe_parks_and_expires`; `c0_3cii_hotplug_while_released_waits_for_acquire`;
`c0_3cii_device_removed_withdraws_and_continues`; `c0_3cii_device_added_is_ignored`;
*(rev 2)* `c0_3cii_removal_while_released_is_not_deferred`,
`c0_3cii_one_failed_probe_applies_nothing` (two devices, one probe fails or is
late), *(rev 3)* `c0_3cii_episode_publishes_once_after_every_commit`,
`c0_3cii_episode_partial_commit_failure` (two devices, one commit rejected, one
unknown; the table's outcomes), `c0_3cii_episode_mixed_legacy_owner`, `c0_3cii_forced_reprobe_timeout_discards_late_result`,
`c0_3cii_udev_events_classified` and `c0_3cii_real_monitor_delivers_typed_events`;
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
server when it is not the renderer; a new card opened; *(rev 2)* a removal
deferred while released; a reinstall submitted on an incarnation whose
release was unknown; a partial episode applied after one device's probe
failed; a timed-out forced reprobe's late result applied; a `DeviceRemoved`
raised from anything but a classified event or `ENODEV`; *(rev 3)* an
unknown release leaving its outputs published; an acquire probe failure that
leaves input paused or ends the server for an Owner device; a mixed acquire
skipping the Legacy resume; an episode publishing one device before another
is terminal.

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
- **3d:** the fresh KMS incarnation for a device whose release was unknown
  (and recovery of a `Poisoned` device); transfer of quarantined release
  resources; `Removed` teardown ordering.
- **Activation (4/5):** one resource service per Owner device (3b-i-2 F15).
- **Out of scope for C.0:** opening a hot-added card.

## 9. For the user

- The release commit carries `ACTIVE=0` only; the cursor detach mentioned
  while brainstorming is stage 4's, as it is for DPMS.
- 3c-i's acquire uses the synchronous probe until 3c-ii replaces every probe
  site — 3c-i can land first without waiting for the probe worker.
- The umbrella's pairing of entry points to kinds is read as the
  classification of §4.2 (by what changed), not a fixed entry-to-kind map.
- *(Rev 3, decided by the coordinator — change it if you disagree.)* An
  acquire whose Owner probe fails withdraws that device and keeps the server
  running, instead of Legacy's exit; input resumes at acquire without waiting
  for the Owner reinstall.
- *(Rev 2.)* A release whose commit is still unknown at the 1 s bound does
  not come back lit after the switch: C.0 §10 forbids reusing that
  incarnation, and creating a fresh one is 3d's recovery work. Between 3c and
  3d this only happens in fixtures and in failure cases (production is Legacy).
